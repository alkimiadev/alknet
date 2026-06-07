# Fail2ban — dev1

## Status

Active. 7 jails. Uses `nftables` backend with `systemd` journal.

## Active Jails

| Jail | Port | Filter | Max Retry | Find Time | Ban Time | Log Source |
|------|------|--------|-----------|-----------|----------|------------|
| sshd | ssh | sshd | default (5) | default (10m) | default (10m) | systemd journal |
| gitea | ssh | gitea | 5 | 10m | 1h | journald (CONTAINER_NAME=gitea) |
| nginx-badbots | http,https | nginx-badbots | 5 | 10m | 1h | /var/log/nginx/access.log |
| nginx-botsearch | http,https | nginx-botsearch | default | default | default | /var/log/nginx/access.log |
| nginx-limit-req | http,https | nginx-limit-req | default | default | default | /var/log/nginx/error.log |
| nginx-401 | http,https | nginx-401 | 5 | 10m | 1h | /var/log/nginx/access.log |
| nginx-403 | http,https | nginx-403 | 10 | 10m | 30m | /var/log/nginx/access.log |

## Configuration

Default settings in `/etc/fail2ban/jail.d/defaults-debian.conf`:

```ini
[DEFAULT]
banaction = nftables
banaction_allports = nftables[type=allports]
backend = systemd
```

Jail configs in `/etc/fail2ban/jail.d/`:
- `gitea.conf` — Gitea jail with Docker journald log driver
- `nginx.conf` — nginx-related jails

## Gitea Jail Details

Gitea runs in Docker with the `journald` log driver. The fail2ban filter uses `journalmatch` to read only Gitea container logs:

```ini
[gitea]
enabled = true
port = ssh
filter = gitea
backend = systemd
journalmatch = CONTAINER_NAME=gitea
maxretry = 5
findtime = 10m
bantime = 1h
action = iptables-allports[chain="DOCKER-USER"]
```

The `DOCKER-USER` chain ensures bans affect Docker traffic.

## Custom Filters

Default install includes `gitea.conf`, `nginx-401.conf`, `nginx-403.conf` in `/etc/fail2ban/filter.d/`. Custom filter:

### nginx-badbots (`/etc/fail2ban/filter.d/nginx-badbots.conf`)

Catches malicious requests that the other nginx jails miss: `.env`/`.git` probes, PROPFIND/CONNECT abuse, common exploit paths (`/actuator`, `/cgi-bin`, `/ecp`, `/SDK`), and binary/garbage requests. Matches 400/404/405/413 status codes for known-bad path patterns only — legitimate 404s (e.g. wrong Gitea repo name) are not matched.

## Lesson Learned: Default Filters Miss Most Scanner Traffic

The default fail2ban nginx filters (`nginx-botsearch`, `nginx-401`, `nginx-403`, `nginx-limit-req`) only catch a narrow subset of malicious requests:

- **nginx-botsearch** only matches `<webmail|phpmyadmin|wordpress|cgi-bin|mysqladmin>` paths returning **404**. Misses `.env`, `.git/config`, `/actuator`, `/SDK`, `/ecp`, crypto mining RPC, PROPFIND/CONNECT abuse, and binary garbage — all of which return 400/405 instead of 404.
- **nginx-401/403** only trigger on those specific status codes. Most scanners get 400 or 405.
- **nginx-limit-req** only triggers when the rate limiter in nginx actually rejects a request.

**Result**: A site with heavy scanner traffic can show zero bans from all four default jails. The `nginx-badbots` custom filter closes this gap by matching known-bad path patterns regardless of status code.

### Verifying Jail Coverage

When setting up fail2ban on a new host:

1. Install jails and filters first
2. Let traffic flow for a few hours
3. Run `sudo fail2ban-regex /var/log/nginx/access.log /etc/fail2ban/filter.d/<filter>.conf` to verify each filter matches expected lines
4. Check `sudo fail2ban-client status` to confirm jails show `Total failed > 0` — if any jail stays at 0 for hours on a public-facing host, the filter likely has a gap
5. Inspect logs manually: `awk '$9>=400' /var/log/nginx/access.log | awk '{print $9}' | sort | uniq -c | sort -rn` shows which status codes scanners are hitting

### Adding the nginx-badbots Filter to a New Host

1. Copy `/etc/fail2ban/filter.d/nginx-badbots.conf` to the new host
2. Append the jail config to `/etc/fail2ban/jail.d/nginx.conf`:

```ini
[nginx-badbots]
enabled = true
port = http,https
filter = nginx-badbots
logpath = /var/log/nginx/access.log
maxretry = 5
findtime = 10m
bantime = 1h
```

3. `sudo fail2ban-client reload`

## Commands

```bash
sudo fail2ban-client status
sudo fail2ban-client status gitea
sudo fail2ban-client set gitea unbanip <IP>
sudo journalctl -u fail2ban -f
```