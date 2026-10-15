# Security notes

This covers OSINTropy 5.0.0, the Rust release in `v5.0.0/`. Report vulnerabilities privately through the repository's security advisory feature: <https://github.com/whisprer-specops/osintropy-community/security/advisories/new>. Include the version, the affected command or source, reproducible steps and the likely impact, using synthetic inputs and redacted credentials.

- Case files may contain personal data. Restrict filesystem access and use an encrypted volume where appropriate. New case files and exports are readable only by their owner on Linux and macOS; on Windows, access follows the folder's permissions.
- Person and organisation searches send the supplied name to the selected source APIs. NPI also receives the optional location. Domain lookups send the requested domain to registry or archive services, not to the target website. The program never sends local imports to a service.
- Connector credentials are read from environment variables; they are not stored in the database or written to reports. OpenCorporates requires its token in the HTTPS query according to its API contract, so request logs outside OSINTropy may contain it.
- Connectors use HTTPS only. Redirects are rejected, each request has 12 seconds in all, and a response may not exceed 2 MB once unpacked. Error messages never repeat a request URL, token or response body. Reports escape HTML and spreadsheet formula prefixes.
- HTTPS certificates are checked by rustls against the Mozilla root certificates built into the program, not against the Windows certificate store. A network that inspects HTTPS with its own root certificate is refused.
- A proxy is used only when `HTTPS_PROXY` is set, and it sees which services you query. HTTP, SOCKS4A and SOCKS5 proxies receive the host names and look them up themselves. A `socks4://` proxy needs addresses, so the program looks the names up locally first; use `socks5h://` or `socks4a://` to keep lookups at the proxy, as Tor needs. If the proxy address cannot be used, searches fail instead of connecting directly. Hosts in `NO_PROXY` are always reached directly.
- Search results for social accounts, articles, cases and historical pages are leads and cannot be linked as verified records. Cross-type links are rejected.
- Review source URLs before opening them. Imported URLs may point to untrusted sites.
- OSINTropy's own code contains no `unsafe` Rust; the crate forbids it. Dependencies are pinned in `Cargo.lock`, and SQLite is compiled from the bundled copy in `rusqlite`. Update dependencies deliberately and run the checks before distributing a build.

No formal external security audit has been performed.
