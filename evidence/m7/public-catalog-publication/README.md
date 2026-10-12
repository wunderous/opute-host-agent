# Public catalog publication

The server now shares the existing publication owner with admission, plan and
recipe validation, audit/redaction and tool-name readers. Each reader retains
an immutable catalog snapshot while it uses descriptors. The MCP tool list and
registered-name lookup use the separate last-completed publication snapshot.
No new capability or provider lifecycle command is added by this checkpoint.

The differential collector exercises 32 cases in both catalog modes over five
iterations. Every transition queries the native authenticated HTTP tools/list
route through the existing modern MCP client; Go uses its actual SDK ListTools.
Inputs, complete parsed observations, source/binary hashes and a capture index
are retained. Comparisons use exact parsed JSON without normalization.

The cases cover retained handles and snapshots, replacement, family displacement,
schema failures, empty services, restoration, retirement and a no-op retirement
following failed displacement. The last case checks that catalog state advances
while the MCP listing retains its prior publication, matching pinned Go.
`../public-catalog-diagnostic/noop-retire` preserves the earlier draft's wire-only
mismatch and the source patch that reconstructs it over commit 1c2b464d.

`../public-catalog-checks/verify.py` independently recomputes publication pairs
and the fresh full public wire matrix and secret sweep, verifies current
source/binary/capture hashes and checks the stale-list regression explicitly.
These captures do not establish public provider installation, executable
invocation, generation-bound Tasks dispatch or complete M7–M13 validation.
