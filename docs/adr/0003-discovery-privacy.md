# ADR 0003: Discovery is local and minimal

Discovery uses DNS-SD/mDNS service `_fileporter._tcp.local.` only on the local network. Advertisements contain protocol and reachability information needed to start pairing, not usernames, paths, file names, transfer activity, or trust decisions.

Discovery is never authorization. Multicast may be unavailable; a validated private `host:port` endpoint remains the manual pairing fallback.
