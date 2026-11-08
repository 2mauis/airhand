# Security

[← Back to README](../README.md) · [简体中文](security.zh-CN.md)

- TLS 1.3 throughout (QUIC); the client verifies the server by **certificate fingerprint pinning**
- Brute-force protection: at most 20 failed attempts per IP (IPv6 grouped by /64) per 10 minutes, and each failure adds a 400 ms delay
- Resume tokens: when a controller connects, the server gives it a token derived from the host's code and secret (it can't be computed or reversed without the secret), and the controller presents it when reconnecting. Requests with a token don't count as failures and aren't rate limited, so however long a host stays offline, automatic reconnects can't lock the controller (or a web console sharing one IP) out. With a wrong token the server always answers "offline", revealing nothing about whether the code exists, so tokens can't be used to guess codes. A token stops working when the host changes its code
- Getting a control code back after a reconnect requires a random secret, so nobody else can claim it
- Fixed control codes: the server only stores a hash of the host's secret (in `airhand.db`), so the database can't be used to claim codes. On Windows the host's secret is encrypted with DPAPI for the SYSTEM account, so a copied `config.toml` doesn't carry the code to another computer; on macOS / Linux `config.toml` is readable by your account only. **Change control code** releases the old code on the server
- My devices holds other computers' control codes: on Windows they're encrypted with your account (DPAPI), on macOS / Linux protected by the file permissions (600). Don't share `config.toml`
- The Windows host runs as SYSTEM so that it can operate UAC prompts and the sign-in screen: whoever has the control code controls the computer with administrator rights. Give the code only to people you trust; whoever sits at the computer sees who is connected and can disconnect them, change the code or quit Airhand
- The host's tray always shows who is controlling the computer; disconnect them or change the code with one click
- The relay server can see the content of session streams (TLS ends at the server), so run it on a machine you trust
- Web console: the WebSocket only accepts pages from the same site (so other websites can't connect through your browser); an access password can be set (HTTP Basic login, then an HttpOnly, SameSite=Strict cookie; wrong passwords are answered with a delay); use HTTPS from other computers. The web console sees the picture too, so run it on a machine you trust. Everyone using it reaches the relay from that machine's IP, so they share the brute-force allowance
