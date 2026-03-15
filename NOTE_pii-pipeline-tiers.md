# PII Pipeline: Tier Wiring

## Detection order (per message text)

```
Tier 1 (regex, sync)  →  spans_t1
Tier 2 (GLiNER ONNX, async, optional)  →  spans_t2
merge(spans_t1, spans_t2)  →  merged  (dedup by overlap, highest-confidence wins)
Tier 3 (SLM HTTP, async, optional):
    high = merged[confidence ≥ slm.confidence_threshold]   → confirmed, skip SLM
    low  = merged[confidence < slm.confidence_threshold]   → send to SLM.disambiguate()
    final = high + confirmed_low  (re-sorted by start)
replace(text, final_spans, vault)
```

## Vault locking

`process_request_body_async` takes `&VaultHandle`, not a pre-locked guard.
The write-lock is acquired **after** all async detection is complete, held only during
the synchronous replacement step, then released. No `MutexGuard` is held across `.await`.

## Tier 2 NER labels

`["person name", "organization", "location", "date of birth", "address"]`

These complement Tier 1 regex patterns. Overlap with Tier 1 hits is harmless
(merge deduplicates by span overlap, keeping the higher-confidence span).

Tier 2 spans are emitted as `PiiType::Custom(label)`, handled by `SyntheticGenerator`
as generic fake data.

## Activation

| Tier | Enabled when |
|------|-------------|
| 1    | Always (when `pii.mode != off`) |
| 2    | `pii.tiers.ner = true` + `ort-ner` feature compiled + model file present |
| 3    | `pii.tiers.slm = true` + `pii.slm.endpoint` non-empty |

## Known limitation: Tier 2 CPU blocking

`Tier2Detector::run_inference` is synchronous CPU work wrapped in `tokio::time::timeout`.
The timeout won't fire during blocking ONNX inference (no yield points inside the loop).
For production use, consider wrapping inference in `tokio::task::spawn_blocking`.

## Config fields added

```toml
[pii.slm]
confidence_threshold = 0.7   # new — spans below this go to SLM; default 0.7
```
