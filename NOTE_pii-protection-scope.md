# NOTE: Scope and Limits of PII Protection

## What this proxy protects against

Privacyclaw's PII pipeline was designed to protect **personal identifiable information** with a fixed structure: credentials, contact details, identity documents. It does this well.

| Information type | Tier | Reliability |
|---|---|---|
| Email addresses | Tier 1 (regex) | High |
| Phone numbers | Tier 1 (regex) | High |
| SSNs, national IDs | Tier 1 (regex) | High |
| API keys, credentials | Tier 1 (regex) | High |
| Bearer tokens, SSH keys | Tier 1 (regex) | High |
| Person names | Tier 2 (GLiNER) | Best-effort |
| Organisation names | Tier 2 (GLiNER) | Best-effort |

---

## What this proxy does NOT reliably protect

### Business-confidential information

A user sharing organisational details with an AI coding tool — company name, employee names, contract amounts, deal terms, business strategy — will find that protection is **partial and unreliable**.

| Information type | Why it leaks |
|---|---|
| Contract amounts (`$50,000`, `2.3M EUR`) | No Tier 1 regex; not a standard NER class in Tier 2 |
| Contract terms and clauses | Semantic meaning, not discrete entities |
| Business strategy and context | Not detectable by any tier |
| Internal codenames and project names | Outside training distribution of general NER models |
| Industry/geography context | Lives in sentence structure, not entity tokens |
| Abbreviations and shorthand | Not flagged as named entities |

### Why Tier 3 (SLM) doesn't close this gap

Tier 3 is a **disambiguator**, not an independent detector. It only confirms or rejects spans already found by Tiers 1 and 2. If Tier 2 didn't flag an entity, the SLM never sees it.

### Implicit identification through context

Even when named entity tokens are replaced with synthetics, the surrounding structure preserves meaning. The sentence:

> "our [ORG] subsidiary in [CITY] is the only EU-licensed provider of [PRODUCT_TYPE]"

remains uniquely identifying. The LLM sees the relationships between replaced entities, not just the tokens.

### The same entity in multiple forms

If a company is called "Acme" in one message and "Acme Corporation" in another, these are detected as separate originals and get different synthetic values. This can confuse the LLM and break conversation coherence, while still not providing full protection.

---

## The threat model mismatch

| Proxy designed for | Organisational use case |
|---|---|
| Personal PII: contact info, credentials, identity numbers | Business confidential: contracts, strategy, financials, proprietary context |
| Discrete named entities with fixed or semi-fixed format | Meaning distributed across sentence structure and domain vocabulary |
| Outbound API traffic sanitisation | Preventing any leakage of competitive or sensitive business information |

These are different threat models. Privacyclaw addresses the first well. It is not a confidentiality boundary for the second.

---

## Additional risks regardless of PII protection

- **Local conversation log is unencrypted.** The full original text (including any PII that was sanitised outbound) is stored in `~/.config/privacyclaw/` in plain NDJSON. Anyone with filesystem access can read it.
- **Images and binary content are not inspected.** Screenshots, documents, or encoded data embedded in requests pass through unchanged.
- **The proxy only sanitises outbound requests.** Provider-side logging, model training data policies, and data retention are outside the proxy's control.

---

## Recommendation

For users concerned about **personal credential and contact data leakage**, Privacyclaw with all tiers enabled provides meaningful protection.

For users concerned about **organisational confidentiality** — contract details, business strategy, proprietary information — the proxy provides only superficial coverage. The appropriate control is to not send that information to an LLM API in the first place, or to use an on-premise / self-hosted model where no data leaves the network boundary.
