## ADDED Requirements

### Requirement: A printed resource URI parses back to itself
The Rust Host Agent SHALL refuse a resource id that contains any Unicode whitespace character, both when it builds a canonical URI and when it parses one (owner decision D30), so that every URI it accepts prints to a string that parses back to the same identity. Whitespace around a whole URI value MAY still be trimmed before it is parsed.

#### Scenario: Unicode whitespace in an id
- **WHEN** a URI is built or parsed with an id that contains U+2003 or U+0085
- **THEN** it is refused with the invalid-resource-URI error, and no resource is registered under a trimmed id

#### Scenario: Accepted URIs round-trip
- **WHEN** any URI the agent accepts is printed and parsed again
- **THEN** the parsed URI equals the original
