# Feature: Model Weight Management for T3 SLM

**Context**: Privacyclaw's T3 tier uses a GGUF model served by `llama-server` for contextual PII detection. The model management infrastructure already has a detailed spec (`openspec/changes/add-macos-packaging/specs/model-management/spec.md`) covering catalog, download, activation, and deletion. This document describes what is still needed specifically for the T3-first pipeline's `/replace` endpoint, and what model weight changes or additions would improve detection quality.

---

## Current state

- A 4-model GGUF catalog is already specced: `smollm2-135m`, `qwen2.5-0.5b`, `llama-3.2-1b`, `phi-3-mini-3.8b`
- Models are downloaded on demand via `/api/models/:id/download` with SHA-256 verification
- The active model is set via `/api/models/:id/activate`, which restarts `llama-server`
- The existing `/disambiguate` endpoint uses the active model with a fixed system prompt
- The new `/replace` endpoint reuses the same active model — **no new weights required for the current feature**

---

## What the current feature does NOT need

The T3-first pipeline (Part I) explicitly reuses the existing GGUF model for the `/replace` endpoint. The prompt changes but the model does not. No new downloads, no new model IDs, no catalog changes.

---

## What future model weight work would look like

### 1. PII-specialized GGUF model

General-purpose instruction-tuned models (Qwen, SmolLM, Phi) are adequate for broad PII detection, but they're not optimized for it. A model fine-tuned on PII detection tasks would produce fewer false negatives on edge cases like:
- Context-dependent secrets (`"my wifi password is hunter2"`)
- Culture-specific ID formats not in the training data
- Implicit PII (`"my brother's apartment, the one near the pharmacy"` — an address by implication)

**What this involves**:
- Selecting a base model (likely `qwen2.5-0.5b` or `llama-3.2-1b` for size/quality tradeoff)
- Fine-tuning on a labeled PII detection dataset (e.g. PII-Masking-400k on HuggingFace, or a custom dataset derived from public sources)
- Quantizing to Q4_K_M GGUF for llama-server compatibility
- Adding the fine-tuned model to the catalog with a new ID (e.g. `privacyclaw-pii-detector-0.5b`)
- Hosting the GGUF file somewhere downloadable with a stable URL + SHA-256

This is significant ML work. Relevant toolchain: `llama.cpp` for quantization, `unsloth` or `axolotl` for fine-tuning, HuggingFace Hub for hosting.

### 2. Bundled starter model

The current flow requires the user to explicitly download a model before T3 works. For a better first-run experience, a very small model could be bundled directly in the release package:

- `smollm2-135m` at Q4_K_M is ~90 MB — feasible to bundle in a `.pkg` installer
- Bundling it removes the "T3 doesn't work until you download a model" gap
- Tradeoff: release tarball grows by 90 MB; detection quality of 135M models is limited

**Packaging impact**: The model file goes into the release tarball under `share/privacyclaw/models/`. The `postinstall` script copies it to `~/Library/Application Support/privacyclaw/models/` and auto-activates it. The Homebrew formula's tarball size increases accordingly.

### 3. Model update notifications

Once models are pinned to specific versions in the catalog, users need a way to know when a better version is available. This implies:
- A catalog version field (`catalog_version`) checked against a remote manifest on startup
- A `privacyclaw models update` CLI command or dashboard notification
- The remote manifest URL must be stable and under project control

This is infrastructure work separate from model weights themselves.

### 4. Per-endpoint model routing

Currently one active model serves both `/disambiguate` and `/replace`. Future state: different endpoints could use different models optimized for their task:

- `/replace`: benefits from a recall-oriented model (catch everything, false positives handled by T1/T2 and T3 disambiguation)
- `/disambiguate`: benefits from a precision-oriented model (only confirm what's clearly PII)

This would require the proxy to manage two `llama-server` instances on different ports, or a single sidecar that routes to different loaded models. Significant architectural change — deferred to a later design discussion.

---

## Near-term recommendation

For the T3-first pipeline launch:
1. No model weight changes — reuse the existing active model
2. Add `smollm2-135m` as the default auto-downloaded model on first run (if no model is active and T3 is enabled, prompt the user or auto-download the smallest catalog model)
3. Document the model selection tradeoff in `docs/PII-SETUP.md` — larger models detect more edge cases at the cost of latency

Bundling a starter model and PII fine-tuning are independent follow-up features, each warranting their own OpenSpec change.

---

## Dependencies on other features

- Requires model management dashboard implementation (specced in `add-macos-packaging`)
- PII fine-tuning requires a labeled dataset and ML infrastructure outside the proxy repo
- Bundled model requires binary bundling decisions (`feature-discussions/packaging-binary-bundling.md`)
