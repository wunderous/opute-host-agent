## ADDED Requirements

### Requirement: A tunnel's CNAME replaces only address records

When the Cloudflare provider points a hostname at a dedicated tunnel, it SHALL
delete only that hostname's A, AAAA and CNAME records that don't already point
at the tunnel, and SHALL keep every other record at the name. If Cloudflare
refuses the CNAME beside a kept record, the call SHALL fail with that error
and delete nothing else.

#### Scenario: A zone apex with mail records
- **WHEN** `ensure_cloudflared_tunnel` routes the apex, which has an old CNAME, MX records and an SPF TXT record
- **THEN** the old CNAME is replaced by the tunnel's CNAME, and the MX and TXT records stay

#### Scenario: A CNAME already in place
- **WHEN** the hostname's CNAME already points at the tunnel
- **THEN** that record is kept and no new one is created
