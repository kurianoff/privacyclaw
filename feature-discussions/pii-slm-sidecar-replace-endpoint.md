# Feature: PII SLM Sidecar — `/replace` Endpoint

**Context**: Privacyclaw is a Rust MITM proxy for LLM traffic. It has a PII protection pipeline that uses a local SLM sidecar (served by `llama-server`) for contextual PII detection. The sidecar currently exposes one custom endpoint: `/disambiguate`. This feature adds a second: `/replace`.

---

## What it does

The `/replace` endpoint receives raw user message text and returns an explicit list of PII strings detected in it — no byte offsets, no modified text, just the strings. The proxy does all the heavy lifting (byte offset resolution, text reconstruction, vault insertion). The sidecar's only job is detection.

## API contract

```
POST /replace
Content-Type: application/json

Request:
{
  "text": "My name is Anne Nicole, my phone is 333-444-5555, safe code 378546",
  "conversation_id": "abc123",
  "entity_start_index": 0
}

Response:
{
  "modified_text": "",        // ignored by proxy — can be empty string
  "replacements": [
    {
      "original": "Anne Nicole",
      "display_value": "",    // ignored by proxy — can be empty string
      "entity_type": "person_name",
      "token_id": "",         // ignored by proxy — can be empty string
      "start": 0,             // byte offset in original text
      "end": 0                // byte offset in original text
    }
  ]
}
```

**Critical simplification**: The proxy ignores `modified_text`, `display_value`, and `token_id` from the response entirely. It computes all of those itself. The sidecar only needs to return a correct `replacements` array with accurate `original` strings.

**Byte offsets**: The sidecar finds `start`/`end` by doing a simple `str.find()` scan on the original text — NOT by asking the LLM. The LLM returns a flat list of detected PII strings; the sidecar resolves offsets deterministically. If the same string appears multiple times, return one entry per occurrence.

## What the LLM prompt should do

Ask the SLM to read the text and return a JSON array of PII strings it detects — names, phone numbers, IDs, secrets, anything sensitive that a user would not want sent to a remote AI. The key value-add over regex is **contextual detection**: things like `"safe code 378546"` where the surrounding text signals sensitivity, not the number format.

Example prompt structure:
```
You are a PII detector. Given the following text, return a JSON array of
all sensitive strings that should be redacted before sending to an AI assistant.
Include: names, phone numbers, emails, IDs, credentials, codes, and any other
value the user likely considers private. Return ONLY a JSON array of strings.

Text: <text>
```

## Failure behavior

If the LLM fails to produce valid JSON, or times out, return HTTP 200 with an empty `replacements` array. The proxy treats empty replacements as "nothing detected" and proceeds to T1/T2 regex/NER. Never return a non-200 on LLM failure — the proxy's fail-open policy depends on an empty-array response.

## Existing sidecar context

- The sidecar already handles `/disambiguate` — same pattern: receive JSON, call LLM, return JSON
- Uses `llama-server` as the model backend (GGUF model, already on disk)
- Language: Python (existing sidecar is Python)
- The existing GGUF model is reused — no new weights needed

## Deliverable

A Python script (`privacyclaw-slm-sidecar`) with both `/disambiguate` (existing) and `/replace` (new) endpoints. The script will be installed by the `postinstall` packaging script — no packaging work needed from you, just the script itself.
