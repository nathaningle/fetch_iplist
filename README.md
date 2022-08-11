# fetch_iplist

Update a file containing a list of IP networks in CIDR notation by downloading from multiple
sources and aggregating.


## Usage

```
fetch_iplist 0.1.0
Nathan Ingle <elgni.nahtan@gmail.com>

USAGE:
    fetch_iplist [OPTIONS] <DESTFILE> <URLS>...

ARGS:
    <DESTFILE>    Path to destination file
    <URLS>...     URLs to download and aggregate

OPTIONS:
    -d, --debug             Verbose logging to the console instead of syslog
    -h, --help              Print help information
    -t, --tempdir <PATH>    Path to directory where temporary files will be created
    -V, --version           Print version information
```

The temporary file and destination file should be on the same filesystem, so that the destination
file is atomically replaced by the temporary file.  (Atomicity of the underlying `rename()` syscall
is strongly hinted at by [POSIX]; there is some conjecture as to whether this constitutes a
guarantee).

Inspired by [acme-client(1)], we can use the exit status to reload some relying service only if the
destination file was updated.  A crontab line might look like:

```
~ * * * * fetch_iplist /etc/pf/blocklist https://example.com/list1.txt https://example.com/list2.txt && pfctl -t blocklist -T replace -f /etc/pf/blocklist
```


## Exit status

Returns 0 if the destination file was updated, 1 on failure, or 2 if the destination file was
already up-to-date.


[acme-client(1)]: https://man.openbsd.org/acme-client.1#EXAMPLES
[POSIX]: https://pubs.opengroup.org/onlinepubs/9699919799/functions/rename.html
