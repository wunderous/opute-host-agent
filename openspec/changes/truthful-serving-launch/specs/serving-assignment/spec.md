## ADDED Requirements

### Requirement: A serving launch is reported only when it started
The Host Agent SHALL report a `process` serving assignment as started only
when its user service supervisor exposes a live main process, and SHALL
otherwise fail the call with the launch's output.

#### Scenario: A system-unit agent launches a process assignment
- **WHEN** an agent running as a system unit reconciles a `process` assignment and the user service supervisor is ready
- **THEN** the process runs under the user service manager and the call reports started, or the call fails with the reason the launch did not start
