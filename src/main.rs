use std::io::Write;
use std::path::PathBuf;

use anyhow::Context;
use clap::{self, Parser};
use ipnet::IpNet;
use reqwest::Url;
use tempfile::NamedTempFile;

#[derive(Parser, Debug)]
#[clap(author, version, about, long_about = None)]
struct Args {
    /// Path to directory where temporary files will be created
    #[clap(short, long, value_parser, value_name = "PATH")]
    tempdir: Option<PathBuf>,
    /// Path to destination file
    #[clap(value_parser)]
    destfile: PathBuf,
    /// URLs to download and aggregate
    #[clap(value_parser = Url::parse, required = true)]
    urls: Vec<Url>,
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();

    if args.destfile.to_str() == Some("-") {
        // If we're writing to stdout, we don't need a temp file.
        let nets: Vec<IpNet> = download_nets(args.urls)?;
        write_nets(std::io::stdout(), &nets)?;
    } else {
        // Set up the temp file early, so we can bail before download if it fails.
        let tmp: NamedTempFile = match args.tempdir {
            Some(dir) => NamedTempFile::new_in(dir),
            None => {
                // Try to create the temp file in the same directory as destfile; if that fails,
                // try to create it in the system-wide default tmp location (probably /tmp).
                args.destfile
                    .parent()
                    .ok_or_else(|| std::io::Error::from(std::io::ErrorKind::NotFound))
                    .and_then(NamedTempFile::new_in)
                    .or_else(|_| NamedTempFile::new())
            }
        }
        .context("Failed to open temporary file")?;
        println!("{:?}", tmp);

        // TODO: warn if temp file and destfile are on different filesystems
        // TODO: log to syslog
        // TODO: handle download failure
        // TODO: copy dest ownership and perms
        let nets: Vec<IpNet> = download_nets(args.urls)?;
        write_nets(&tmp, &nets)?;
        tmp.as_file().sync_all()?;
        let final_destfile = tmp.persist(args.destfile)?;
        final_destfile.sync_all()?;
    }

    Ok(())
}

// Write prefixes one per line in CIDR notation to something 'Write'able.
fn write_nets(mut dest: impl Write, nets: &[IpNet]) -> std::io::Result<()> {
    let mut cidr_list: String = nets
        .iter()
        .map(|net| net.to_string())
        .collect::<Vec<String>>()
        .join("\n");
    cidr_list.push('\n');
    dest.write_all(cidr_list.as_bytes())?;
    dest.flush()
}

// Fetch and aggegate lists of prefixes.
fn download_nets(urls: Vec<Url>) -> anyhow::Result<Vec<IpNet>> {
    let webclient = reqwest::blocking::Client::new();
    let bodies: Vec<String> = urls
        .into_iter()
        .map(|url| webclient.get(url).send().and_then(|resp| resp.text()))
        .collect::<Result<Vec<String>, reqwest::Error>>()?;

    let nets: Vec<IpNet> = bodies.iter().flat_map(|body| extract_nets(body)).collect();
    Ok(IpNet::aggregate(&nets))
}

// True iff a character would be expected in an IPv4 or IPv6 network address.
fn is_net_char(c: char) -> bool {
    c.is_ascii_hexdigit() || c == '.' || c == ':' || c == '/'
}

// Strip a string down to an IPv4 or IPv6 address by removing:
//   * leading whitespace, then
//   * the first non-IP character and everything following it
fn just_the_net(s: &str) -> &str {
    let trimmed_s = s.trim_start();
    trimmed_s
        .split_once(|c| !is_net_char(c))
        .map_or(trimmed_s, |tup| tup.0)
}

// Find IPv4 and IPv6 network addresses in a string, silently discarding everything else.
fn extract_nets(s: &str) -> Vec<IpNet> {
    s.lines()
        .filter_map(|l| just_the_net(l).parse().ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_just_the_net() {
        // Use prefixes from RFCs 3849 and 5737.
        assert_eq!(just_the_net("192.0.2.0/24"), "192.0.2.0/24");
        assert_eq!(just_the_net("    192.0.2.0/24"), "192.0.2.0/24");
        assert_eq!(just_the_net("    192.0.2.0/24 pelican"), "192.0.2.0/24");
        assert_eq!(
            just_the_net("2001:db8:1234:5678:90ab:cdef::/96"),
            "2001:db8:1234:5678:90ab:cdef::/96"
        );
        assert_eq!(
            just_the_net("    2001:db8:1234:5678:90ab:cdef::/96"),
            "2001:db8:1234:5678:90ab:cdef::/96"
        );
        assert_eq!(
            just_the_net("    2001:db8:1234:5678:90ab:cdef::/96 pelican"),
            "2001:db8:1234:5678:90ab:cdef::/96"
        );
        assert_eq!(just_the_net("    pelican"), "");
    }

    #[test]
    fn test_extract_nets() {
        assert_eq!(
            extract_nets("192.0.2.0/24\n2001:db8:1234:5678:90ab:cdef::/96\n"),
            vec![
                "192.0.2.0/24".parse::<IpNet>().unwrap(),
                "2001:db8:1234:5678:90ab:cdef::/96"
                    .parse::<IpNet>()
                    .unwrap()
            ]
        );
        assert_eq!(
            extract_nets("  192.0.2.0/24\n\n# comment\n2001:db8:1234:5678:90ab:cdef::/96\n"),
            vec![
                "192.0.2.0/24".parse::<IpNet>().unwrap(),
                "2001:db8:1234:5678:90ab:cdef::/96"
                    .parse::<IpNet>()
                    .unwrap()
            ]
        );
    }

    #[test]
    fn test_write_nets() {
        let nets = vec![
            "192.0.2.0/24".parse::<IpNet>().unwrap(),
            "2001:db8:1234:5678:90ab:cdef::/96"
                .parse::<IpNet>()
                .unwrap(),
        ];
        let mut buf: Vec<u8> = Vec::new();
        write_nets(&mut buf, &nets).unwrap();
        assert_eq!(
            std::str::from_utf8(&buf).unwrap(),
            "192.0.2.0/24\n2001:db8:1234:5678:90ab:cdef::/96\n"
        );
    }
}
