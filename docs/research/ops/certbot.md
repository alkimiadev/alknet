# Certbot — dev1

## Overview

Let's Encrypt SSL certificates managed by certbot. Used by nginx for HTTPS.

## Installed

certbot (snap package on Ubuntu 24.04)

## Certificates

| Domain | Expiry | Path |
|--------|--------|------|
| git.alk.dev | 2026-06-18 | /etc/letsencrypt/live/git.alk.dev/ |

## File Locations

```
/etc/letsencrypt/live/git.alk.dev/
├── fullchain.pem    # Server cert + chain
├── privkey.pem      # Private key
├── cert.pem         # Server cert only
├── chain.pem        # Chain only
└── README
```

Renewal config: `/etc/letsencrypt/renewal/git.alk.dev.conf`

## Renewal

Certbot auto-renews via systemd timer. Certificates renew when <30 days remaining.

```bash
# Check certificates and expiry
sudo certbot certificates

# Dry run renewal
sudo certbot renew --dry-run

# Force renewal (if needed)
sudo certbot renew --force-renewal

# Reload nginx after renewal
sudo systemctl reload nginx
```

## Initial Certificate

If adding a new domain, obtain the cert with the standalone plugin (nginx doesn't need to be running):

```bash
sudo certbot certonly --standalone -d <domain> --agree-tos -m <email>
```

Port 80 must be open for the ACME challenge. The api.alk.dev UFW rule allows HTTP for this purpose.