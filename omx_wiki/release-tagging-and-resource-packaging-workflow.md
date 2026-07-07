---
title: "Release Tagging and Resource Packaging Workflow"
tags: ["release", "tags", "github-actions", "package-tag", "settings-schema", "oss"]
created: 2026-07-07T07:06:16.351Z
updated: 2026-07-07T07:06:16.351Z
sources: []
links: []
category: reference
confidence: medium
schemaVersion: 1
---

# Release Tagging and Resource Packaging Workflow

# Release Tagging and Resource Packaging Workflow

Captured: 2026-07-07.

## Required Tag Format

`script/validate_release_tag` enforces the repository release-tag format:

`v<major>.<YYYY>.<MM>.<DD>.<HH>.<mm>.<channel>_<NN>`

Allowed channels are `oss`, `stable`, `preview`, and `dev`. Example: `v0.2026.07.01.00.00.oss_00`.

Do not use `github-*` or other ad hoc tag formats for package releases. The tag validation script is the source of truth.

## GitHub Actions Flow

`.github/workflows/package_tag.yml` runs on tag pushes. It validates the tag with `script/validate_release_tag`, extracts the release channel from the tag, and passes the channel into `script/prepare_bundled_resources` for common and platform-specific release resources.

The workflow builds `generate_settings_schema` and runs it with the selected channel. The settings schema generator should recognize all release channels used by the workflow, including `oss`, to avoid defaulting unexpectedly.

## Bundled Resources

`script/prepare_bundled_resources` prepares common resources and platform-specific resources. For Windows, the workflow copies common bundled resources, writes version metadata, copies channel-gated skills, generates third-party licenses, and copies cached settings schema.

Recent observed CI output included `Unknown channel 'oss', defaulting to dev` from `generate_settings_schema --channel oss` and SPDX parser errors for deprecated `GPL-2.0` license identifiers during third-party license generation. The unknown-channel message is a channel-handling issue; the GPL output is from license expression parsing and may require replacing deprecated identifiers with SPDX-valid expressions where generated/license metadata is sourced.

## Channel Surface

`crates/warp_core/src/channel/mod.rs` includes `Channel::Oss`. The OSS channel uses CLI names such as `warp-oss`/`warpctrl-oss`, displays as `warp-oss`, is not dogfood, and does not allow server URL overrides.

## Invariants

- Release tags must pass `script/validate_release_tag` before pushing.
- Select replacement tags in the required format before deleting/replacing invalid remote tags.
- Keep workflow channel parsing and application channel enums in sync.
- Settings schema generation must not silently fall back to dev for a valid `oss` release.
- Treat license-generation errors as release-quality issues even if resource preparation continues.
