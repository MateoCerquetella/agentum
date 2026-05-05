# Remote access

agentum is a local tool that you can also reach over the network — from
your phone, another laptop, or a friend you want to share access with.
There's no third-party tunnel involved: connections go straight to the
host running `agentum serve`.

## The connection model

Every connection looks the same:

```
┌─────────────┐     HTTPS :8822     ┌────────────────────────┐
│  browser    │ ──────────────────► │  agentum serve         │
│  / TUI      │                     │  (host running tmux)   │
└─────────────┘     bearer token    └────────────────────────┘
```

What changes between "LAN", "public IP", and "VPN" is **which network
path** the browser/TUI uses to reach the host. The protocol, auth, and
TLS behaviour are identical.

## Three ways to be on the same network as the host

### 1. Same LAN

You and the host are on the same Wi-Fi / Ethernet network. The host's
LAN IP looks like `192.168.x.y` or `10.x.y.z`.

- **From the second device's browser**: paste
  `https://<host-lan-ip>:8822`.
- **From the second device's TUI**:
  `agentum terminal --api https://<host-lan-ip>:8822`.

No router config needed.

### 2. Public IP / port forward

You want to reach the host from the public internet (cellular phone,
remote laptop) and you control the host's router or it has a public
IPv4 directly (e.g. a VPS).

- Forward port `8822/tcp` from the router to the host. (On a VPS that's
  already done — the box has a public IP.)
- Share `https://<public-ip-or-domain>:8822`.

This is the same shape as exposing any HTTPS service. The threats and
mitigations are documented in [SECURITY](#security) below.

### 3. VPN (WireGuard / Tailscale)

You and the second device join the same overlay network. From the
host's perspective, the second device looks like it's on its LAN.

- Share `https://<host-vpn-ip>:8822` — the address the host has on the
  VPN, *not* its public address.
- Connection only works while the second device's VPN is up.

This is the most private of the three (the host stays unreachable from
the open internet) but every device that wants access has to install
and configure the VPN client.

## First-time setup, step by step

1. **On the host**, run `agentum serve`. Two important lines appear in
   the terminal:

   ```
   bootstrap PIN: 12345678
   TLS cert fingerprint (verify on second device): SHA-256 AB:CD:…
   ```

   Keep that terminal open — you'll need both values in the next steps.

2. **From any browser on the network** (your laptop, your phone), open
   `https://<host-ip>:8822`. The browser warns about a self-signed cert
   — accept it.

3. The dashboard's onboarding wizard kicks in. It walks you through:
   - **Create admin** — username, password, plus the bootstrap PIN
     from step 1. The PIN closes the LAN race window where someone
     else on the network could grab the admin slot first.
   - **Verify cert** — the wizard shows the SHA-256 fingerprint the
     server is presenting. Confirm it matches the `SHA-256 AB:CD:…`
     line in the host terminal. If they differ, **someone is
     intercepting the connection** — abort and investigate.
   - **Share access** — the wizard ends on a panel with the URL and
     fingerprint, plus a copy-to-clipboard button for each. Send
     these out-of-band (Signal, AirDrop, paper) to anyone you want
     on this server.

4. **Other devices** repeat step 2, then log in with the credentials
   you just made. They'll go through their browser's "trust this
   self-signed cert" flow once. iOS Safari is the most painful case
   — see [iOS notes](#ios-notes) below.

5. **Adding more users**: from the host terminal, run
   `agentum auth add <username>` (it prompts for a password). The
   anonymous registration endpoint is closed once the first user
   exists, so additional accounts come from the CLI, not the dashboard.

## Connecting the TUI to a remote agentum

```sh
agentum terminal --api https://my-vps:8822
```

The TUI uses **SSH-style fingerprint pinning**. First contact with a
new host shows the cert fingerprint and asks you to confirm it matches
what `agentum serve` printed. Once accepted, it's saved to
`$XDG_CONFIG_HOME/agentum/known_hosts.toml` (mode 0600). Future
connects verify the pin silently; a mismatch refuses to connect.

To skip the prompt (CI, scripts, copy-paste from a doc):

```sh
agentum terminal --api https://my-vps:8822 --fingerprint AB:CD:…
```

To inspect or drop pinned hosts:

```sh
agentum hosts list
agentum hosts forget my-vps:8822
```

## iOS notes

iOS doesn't make trusting self-signed certs easy:

1. On Safari, open `http://<host-ip>:8823/api/cert` (the plain-HTTP
   sidecar — not 8822). Safari downloads the PEM as a *configuration
   profile*.
2. Settings → General → VPN & Device Management → install the profile.
3. Settings → General → About → Certificate Trust Settings → enable
   full trust for the agentum cert.
4. Now `https://<host-ip>:8822` opens cleanly.

If that's too much friction, run agentum on a host that has a real
domain + Let's Encrypt cert in front of it (any reverse proxy works:
Caddy, nginx, Traefik). The fingerprint check then collapses into the
browser's regular CA verification.

## Security

A public-facing agentum (option 2) takes credentials over a self-signed
cert. The recent hardening lands relevant defenses:

- Online password guessing: per-IP rate limit of 8 login attempts per
  5 minutes, then 429.
- First-user race: bootstrap PIN required on the very first
  registration, single-use, lives only in process memory.
- Cert MITM: SHA-256 fingerprint printed on every `agentum serve` boot
  so it can be verified out-of-band.
- Token theft via logs: `?token=…` is redacted from access logs.
- Sliding 30-day token expiry; expired rows swept hourly.

It's fine for trusted users on the open internet, not for "anyone with
a guess at the URL". If you're worried about the latter, put it behind
a VPN (option 3) or a real reverse proxy with a CA-issued cert.
