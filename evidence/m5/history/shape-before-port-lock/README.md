# Rejected pre-lock shape run

This run completed 418/420 comparisons and is not acceptance evidence.
The two failures were defaults.port-platform.3 and defaults.port-standalone.3.
The Go sides recorded ready=true, an empty listener list, SIGINT exit -2 and
no authorization database. The Rust sides recorded their expected default
listener, graceful exit and the initialized database.

Concurrent test suites could previously mark a process ready when any process
opened the target port. Readiness now requires the tested PID to own the socket;
fixed-port scenarios also acquire a cross-process lock shared by capture and
twin/shape runs. Negative controls verify foreign-listener rejection and lock
blocking/release. All accepted evidence must be regenerated under that harness.
