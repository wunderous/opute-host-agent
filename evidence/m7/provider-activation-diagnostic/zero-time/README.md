# Verifier correction for zero activation time

This historical capture exposed an assumption in the initial differential verifier. A restored generation can be active with Go's zero `time.Time`. Generation JSON represents that time as `0001-01-01T00:00:00Z`, while `persistProviderGeneration` deliberately writes an empty activation-time string. The initial consistency assertion required those two representations to be identical.

`verifier-before.py` and the raw batch preserve the failed assertion's inputs and observations. This was a verifier error, with no native runtime mismatch established. The current driver and independent verifier account for the pinned zero-time conversion before checking consistency, and still require nonzero activation times to agree exactly across generation and durable records. Fresh clock values are normalized only after clock-bound checks.
