## ADDED Requirements

### Requirement: CA Initialization

On first run via `privacyclaw init`, the system SHALL generate a root CA key pair (ECDSA P-256) and self-signed root certificate and store them in a platform-specific secure location (`~/Library/Application Support/privacyclaw/ca/` on macOS, `~/.config/privacyclaw/ca/` on Linux, `%APPDATA%\privacyclaw\ca\` on Windows).

#### Scenario: First-time initialization

- **WHEN** the user runs `privacyclaw init` and no CA exists
- **THEN** a new ECDSA P-256 key pair and self-signed certificate are generated
- **AND** they are saved to the platform-appropriate directory
- **AND** the CA certificate path is printed to stdout

#### Scenario: Re-use existing CA

- **WHEN** the user runs `privacyclaw init` and a CA already exists
- **THEN** the existing CA is loaded without regeneration
- **AND** the user is notified that the existing CA was found

### Requirement: CA Trust Store Installation

The system SHALL offer to install the CA certificate into the OS trust store via `privacyclaw init --install-ca`, with platform-specific commands.

#### Scenario: macOS trust store installation

- **WHEN** the user runs `privacyclaw init --install-ca` on macOS
- **THEN** the system runs `security add-trusted-cert` with the CA certificate
- **AND** reports success or failure with instructions for manual installation

#### Scenario: Installation declined or failed

- **WHEN** the user declines automatic installation or the command fails
- **THEN** clear manual installation instructions are printed

### Requirement: CA Reset

The system SHALL support `privacyclaw reset-ca` to delete the existing CA and generate a new one.

#### Scenario: CA reset

- **WHEN** the user runs `privacyclaw reset-ca`
- **THEN** existing CA files are deleted
- **AND** a new CA key pair and certificate are generated and saved

### Requirement: Dynamic Leaf Certificate Generation

The proxy SHALL dynamically generate TLS leaf certificates for intercepted domains, signed by the local CA, and cache them in memory by domain name.

#### Scenario: First request to a domain

- **WHEN** a CONNECT request arrives for an allowlisted domain with no cached leaf cert
- **THEN** a new leaf cert is generated using rcgen, signed by the CA
- **AND** the cert is cached in memory for subsequent requests to the same domain

#### Scenario: Cached cert reuse

- **WHEN** a CONNECT request arrives for a domain with an existing cached leaf cert
- **THEN** the cached cert is used without regeneration
