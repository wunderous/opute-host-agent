#![no_main]

libfuzzer_sys::fuzz_target!(|data: &[u8]| opute_host_agent::fuzzing::provider_descriptor(data));
