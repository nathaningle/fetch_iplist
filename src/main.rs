use std::io::Write;
use std::path::PathBuf;

use anyhow::{ensure, Context};
use clap::{self, Parser};
use ipnet::IpNet;
use log::{debug, info, warn, LevelFilter};
use nix::sys::stat::{fchmodat, lstat, mode_t, FchmodatFlags, FileStat, Mode, SFlag};
use nix::unistd::{chown, Gid, Uid};
use reqwest::Url;
use simple_logger::SimpleLogger;
use syslog::{BasicLogger, Facility, Formatter3164};
use tempfile::NamedTempFile;

#[derive(Parser, Debug)]
#[clap(author, version, about, long_about = None)]
struct Args {
    /// Verbose logging to the console instead of syslog
    #[clap(short, long, action)]
    debug: bool,
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

    // Init logging.
    if args.debug {
        SimpleLogger::new()
            .with_level(LevelFilter::Debug)
            .init()
            .expect("simple_logger failed to initialise");
    } else {
        let formatter = Formatter3164 {
            facility: Facility::LOG_USER,
            hostname: None,
            process: "fetch_iplist".into(),
            pid: 0,
        };
        let logger = syslog::unix(formatter).expect("syslog failed to initialise");
        log::set_boxed_logger(Box::new(BasicLogger::new(logger)))
            .map(|()| log::set_max_level(LevelFilter::Info))?;
    }

    if args.destfile.to_str() == Some("-") {
        // If we're writing to stdout, we don't need a temp file.
        debug!("Writing to stdout");
        let nets: Vec<IpNet> = download_nets(args.urls)?;
        write_nets(std::io::stdout(), &nets)?;
    } else {
        debug!("Writing to {} via temporary file", args.destfile.display());

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
        debug!("Opened temporary file {}", tmp.path().display());

        // Read file metadata.
        let dest_stat: FileStat = lstat(&(args.destfile))?;
        let tmp_stat: FileStat = lstat(tmp.path())?;

        // Sanity checks.
        ensure!(
            !is_symlink(&dest_stat),
            "Destination file must not be a symbolic link"
        );
        if tmp_stat.st_dev != dest_stat.st_dev {
            warn!("Destination and temporary files must be on the same filesystem to guarantee atomic replacement");
        }

        // Make tempfile ownership the same as destfile.
        let dest_uid: Option<Uid> =
            (tmp_stat.st_uid != dest_stat.st_uid).then(|| Uid::from_raw(dest_stat.st_uid));
        let dest_gid: Option<Gid> =
            (tmp_stat.st_gid != dest_stat.st_gid).then(|| Gid::from_raw(dest_stat.st_gid));
        if dest_uid.is_some() || dest_gid.is_some() {
            debug!(
                "Updating temporary file ownership to user {:?}, group {:?}",
                dest_uid, dest_gid
            );
            chown(tmp.path(), dest_uid, dest_gid)?;
        }

        // Download and aggregate the prefix lists.
        let nets: Vec<IpNet> = download_nets(args.urls)?;
        debug!("Writing network prefixes to temporary file");
        write_nets(&tmp, &nets)?;
        // TODO: stop here if tempfile == destfile

        // Make tempfile permissions the same as destfile.  Do this after writing to the tempfile
        // in case we would make it read-only.
        if tmp_stat.st_mode != dest_stat.st_mode {
            let dest_mode: Mode = Mode::from_bits_truncate(dest_stat.st_mode);
            debug!("Updating temporary file permissions to {:?}", dest_mode);
            fchmodat(None, tmp.path(), dest_mode, FchmodatFlags::NoFollowSymlink)?
        }

        // Move the tempfile over the top of the destination file.
        tmp.as_file().sync_all()?;
        debug!("Moving temporary file to {}", args.destfile.display());
        let final_destfile = tmp.persist(&args.destfile)?;
        final_destfile.sync_all()?;
        info!("Updated {}", args.destfile.display());
    }

    Ok(())
}

// True iff the given FileStat is from a symbolic link.
fn is_symlink(fs: &FileStat) -> bool {
    SFlag::from_bits_truncate(fs.st_mode as mode_t).contains(SFlag::S_IFLNK)
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
        .map(|url| {
            webclient
                .get(url)
                .send()
                .and_then(|resp| resp.error_for_status())
                .and_then(|resp| resp.text())
        })
        .collect::<Result<Vec<String>, reqwest::Error>>()?;

    let nets: Vec<IpNet> = bodies.iter().flat_map(|body| extract_nets(body)).collect();
    let agg_nets = IpNet::aggregate(&nets);
    info!(
        "Downloaded {} network prefixes, aggregated to {}",
        nets.len(),
        agg_nets.len()
    );
    Ok(agg_nets)
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
