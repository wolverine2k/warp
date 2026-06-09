# LocalLLM Changelog

This file is the curated release-note source for semver localLLM tags. The
package-tag workflow uses a matching tag section first, then a populated
`Unreleased` section. If neither has content, semver tags fail instead of
publishing an uncurated commit dump.

## Unreleased

## v1.4.0
#### New Features
- Added Gemini CLI as a supported local child harness, including BYOP API key and endpoint injection through Gemini `settings.json`.
- Added BYOP orchestration model choices for local child agents, with harness-aware filtering and submit-time validation.
- Added BYOP credential forwarding for Remote workers, including managed-secret selection, auto-create support, and AgentConfig snapshot fields.

#### Improvements
- Threaded BYOP environment variables through local harness launches so external CLIs receive the selected provider configuration.
- Forwarded BYOP and compaction configuration into Remote worker launches while preserving existing localLLM behavior after the master merge.
- Updated the localLLM README and Phase 5 design/spec documents to cover Gemini, Remote credential bridging, and worker-side BYOP contracts.

#### Bug Fixes
- Preserved custom provider models during LLM preference reconciliation and custom endpoint usage display paths.
- Kept localLLM test coverage aligned with the provider-secrets model introduced by the master merge.
