## 1. Tests first

- [x] 1.1 `providers::cloudflare::tests::a_tunnel_cname_keeps_the_apex_mail_records`: an apex with an old CNAME, A, AAAA, MX, TXT and the tunnel's CNAME. With 0.5.5's rule the MX and TXT records are deleted too.

## 2. Change

- [x] 2.1 The CNAME upsert deletes only A, AAAA and CNAME records that don't point at the tunnel.

## 3. Evidence

- [x] 3.1 www.opute.io and opute.io are served from the dev machine's K3s by `docs/recipes/opute-io-site.recipe.yaml`, and the apex keeps its MX and TXT records (`evidence/m34`).
