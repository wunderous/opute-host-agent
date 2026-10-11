# Connected activation composition

Twenty-four isolated cases, in both supported catalog modes, run five times:
**120/120 comparisons pass** against the clean pinned Go activation owner.
Each batch owns an OS-assigned loopback listener; both implementations connect
to its same endpoint. The provider fixture supports modern discovery, listing
and synchronous probes. No domain operation or shared runtime is touched.

Coverage includes activation and idempotent repetition, dependency chains,
first-service and partial-mount failures, failed replacement restoring the prior
mount and connection, successful replacement closing the predecessor,
reaffirmation against an existing mount, admission denial, closed durable store,
recipe capability filtering, completion before activation, explicit candidate
cleanup and repeated cleanup/unmount. Probes use retained connected adapters;
they do not claim public admitted operation dispatch or generation affinity.

Each capture retains input, complete Go/native observations, actual HTTP
requests and real-clock bounds. Catalogs, durable rows, events, candidate adapter
ownership, service mount order and dependency bindings are compared structurally.
Fresh event/activation times are masked only after bounds and durable/active
consistency checks. HTTP payloads, paths and protocol/client/auth header values
are compared separately; raw headers and body bytes are retained. This is not a
claim of full raw HTTP byte equality.

`../provider-activation-connected-checks/verify.py` independently recomputes the
pairs and checks specific successful mounting, rollback and closed predecessor
outcomes. It validates source/binary/capture hashes and exact loopback endpoint
substitution. It also rechecks the existing public captures and secret sweep
only after unchanged public release bytes and harness identity are verified.

This receipt supersedes no historical private receipt's source hashes. The
previous metadata-only activation receipt is historical after fixture/overlay
changes. Public lifecycle wiring, enclosing recipe cleanup, supervised provider
executables, restart/relaunch and full M7–M13 validation remain open. Pending
contract proposals and owner-only cutover decisions remain unchanged.
