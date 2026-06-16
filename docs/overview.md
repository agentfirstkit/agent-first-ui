# AFUI Overview

Agent-First UI (AFUI) is a data-only protocol for operation surfaces. Producers describe workflow state, semantic views, action declarations, risks, and patches. Trusted hosts render those documents and return structured user intent as `user_action` messages.

## Roles

- Producer: emits `ui_snapshot`, `state_patch`, `document_patch`, and `theme_patch` messages and handles `user_action`.
- Host: renders AFUI, enforces safety policy, resolves bindings, redacts secrets, applies patches, and emits user intent.
- Composer: combines several producer surfaces by namespacing ids/actions/state and outputting a normal `ui_snapshot`.

## Data Boundary

Ordinary AFUI data must not contain executable or installable code: no JavaScript, CSS, HTML, WASM, package manifests, build scripts, shell commands, callback bodies, expressions, or remote component/plugin URLs.

Unknown view kinds are inert semantic data. Hosts may render them with fallback or with preinstalled trusted capabilities, but documents never cause a host to download, build, or execute code.

## What ships here

This repository is the protocol itself: the normative spec, the JSON schema, conformance fixtures, examples, and a lightweight Rust SDK (`rust/`, the `agent-first-ui` crate) that producers use to emit and validate AFUI data. The SDK carries no rendering code, so a producer can depend on it without pulling in any browser, terminal, or server code.

A host — trusted local software you own — renders AFUI data. The protocol defines what a conforming host must do; it does not bundle one.
