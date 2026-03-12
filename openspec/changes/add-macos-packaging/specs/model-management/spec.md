## ADDED Requirements

### Requirement: GGUF Model Catalog

The system SHALL maintain a built-in catalog of supported GGUF models with metadata: name, description, Q4 file size, estimated RAM requirement, HuggingFace download URL, and expected SHA-256 checksum. The initial catalog SHALL contain:

| ID | Model | Q4 size | RAM |
|---|---|---|---|
| `smollm2-135m` | SmolLM2-135M-Instruct | ~90 MB | ~300 MB |
| `qwen2.5-0.5b` | Qwen2.5-0.5B-Instruct | ~400 MB | ~800 MB |
| `llama-3.2-1b` | Llama-3.2-1B-Instruct | ~700 MB | ~1.2 GB |
| `phi-3-mini-3.8b` | Phi-3-mini-4k-instruct | ~2.3 GB | ~3.5 GB |

#### Scenario: Catalog endpoint returns all models

- **WHEN** a GET request is made to `/api/models`
- **THEN** a JSON array is returned with all catalog entries including `id`, `name`, `size_bytes`, `ram_bytes`, `downloaded` (bool), `active` (bool), and `download_progress` (0–100 or null)

### Requirement: Model Download

The system SHALL support downloading a catalog model on demand. Download progress SHALL be streamed to all connected WebSocket clients as `model_download_progress` events. The download SHALL be verified against the expected SHA-256 checksum after completion. Partial files SHALL be deleted on failure or cancellation.

#### Scenario: Initiate download

- **WHEN** a POST request is made to `/api/models/:id/download`
- **AND** the model is not yet downloaded
- **THEN** the download begins in the background and a 202 Accepted response is returned

#### Scenario: Progress streamed via WebSocket

- **WHEN** a model download is in progress
- **THEN** `model_download_progress` WebSocket events are broadcast every 2 seconds (or on significant chunk) with `{ "model_id": "...", "progress": 42, "bytes_downloaded": ..., "bytes_total": ... }`

#### Scenario: Checksum failure

- **WHEN** a download completes but the SHA-256 does not match the catalog value
- **THEN** the partial file is deleted and a `model_download_error` WebSocket event is sent with a descriptive message

#### Scenario: Cancel download

- **WHEN** a DELETE request is made to `/api/models/:id/download`
- **AND** a download is in progress
- **THEN** the download is aborted and the partial file is deleted

### Requirement: Model Activation

The system SHALL allow selecting one downloaded model as the active Tier 3 model. Activation SHALL update `pii.slm.model_id` in the running config and on disk, stop the currently running `llama-server` sidecar if any, and start a new sidecar with the selected model on port 16442.

#### Scenario: Activate downloaded model

- **WHEN** a POST request is made to `/api/models/:id/activate`
- **AND** the model file is present on disk
- **THEN** the existing llama-server sidecar is stopped (if running)
- **AND** a new sidecar is started with the selected model
- **AND** Tier 3 PII processing uses the new model for subsequent requests

#### Scenario: Activate undownloaded model

- **WHEN** a POST request is made to `/api/models/:id/activate`
- **AND** the model has not been downloaded
- **THEN** a 409 Conflict response is returned with `"error": "model not downloaded"`

#### Scenario: Deactivate (no active model)

- **WHEN** a POST request is made to `/api/models/deactivate`
- **THEN** the running sidecar is stopped and Tier 3 is disabled until a model is activated

### Requirement: Model Storage Management

The system SHALL report disk usage for downloaded models and allow deleting a model to free space. A model that is currently active SHALL NOT be deleted without first deactivating it.

#### Scenario: Delete downloaded model

- **WHEN** a DELETE request is made to `/api/models/:id`
- **AND** the model is not the currently active model
- **THEN** the GGUF file is deleted from `~/.config/claudovka/models/`
- **AND** the catalog entry is updated to `downloaded: false`

#### Scenario: Delete active model rejected

- **WHEN** a DELETE request is made to `/api/models/:id`
- **AND** the model is currently active
- **THEN** a 409 Conflict response is returned with `"error": "deactivate model before deleting"`
