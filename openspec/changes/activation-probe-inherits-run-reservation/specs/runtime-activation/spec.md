## ADDED Requirements

### Requirement: An activation probe is admitted under its run

When a recipe run with `activate: true` completes and its serving contract's
validation flow calls a tool, the Host Agent SHALL dispatch that call in the
run's context, so it is admitted under the run's reservation as the run's nodes
are. It SHALL NOT refuse the call for capacity that the run itself holds.

#### Scenario: An openai-chat.v1 recipe is activated
- **WHEN** a runtime recipe with an `openai-chat.v1` activation completes and its bound endpoint streams a chat completion
- **THEN** the run is `completed` and `activeRuntime` names the recipe's capability with `observation.streamingChatReady: true`
