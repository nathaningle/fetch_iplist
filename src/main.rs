use std::path::PathBuf;

use clap::{self, Parser};
use ipnet::IpNet;
use reqwest::Url;

#[derive(Parser, Debug)]
#[clap(author, version, about, long_about = None)]
struct Args {
    #[clap(value_parser)]
    destfile: PathBuf,
    #[clap(value_parser = Url::parse, required = true)]
    urls: Vec<Url>,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();

    let webclient = reqwest::blocking::Client::new();
    let bodies: Vec<String> = args
        .urls
        .into_iter()
        .map(|url| webclient.get(url).send().and_then(|resp| resp.text()))
        .collect::<Result<Vec<String>, reqwest::Error>>()?;

    let nets = bodies.iter().flat_map(|body| extract_nets(body)).collect();

    for net in IpNet::aggregate(&nets) {
        println!("{}", net);
    }

    Ok(())
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
}
