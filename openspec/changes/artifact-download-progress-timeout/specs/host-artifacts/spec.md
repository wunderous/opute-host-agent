## ADDED Requirements

### Requirement: Artifact downloads are limited by progress and total time

When the Host Agent downloads an artifact for `ensure_host_artifact`, or for
the public MCP connector install, it SHALL fail the download when no bytes
arrive for 300 seconds while connecting, awaiting the response headers or
reading the body, and when the download has not finished within one hour. It
SHALL NOT otherwise limit how long a download that keeps making progress may
take. The 4 GiB size cap, the HTTPS-only redirects and the SHA-256 check are
unchanged, and nothing is installed from a failed download.

#### Scenario: A large artifact on a slow link
- **WHEN** an artifact keeps arriving for longer than 300 seconds and finishes within one hour
- **THEN** the download completes and the artifact is installed after its SHA-256 matches

#### Scenario: A stalled transfer
- **WHEN** no bytes of the body arrive for 300 seconds
- **THEN** the call fails with `read host artifact: timed out reading response` and no file is installed at the destination

#### Scenario: A transfer that never finishes
- **WHEN** a server keeps sending bytes but the download has not finished after one hour
- **THEN** the call fails and no file is installed at the destination
