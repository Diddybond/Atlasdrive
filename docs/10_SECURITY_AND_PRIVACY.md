# 10. Security and Privacy

## Threat model

The application protects private family images and biometric-derived data on a personal Mac. Primary risks include:

- accidental modification or deletion of originals
- catalogue exposure to another local account or stolen device
- unapproved network transmission
- corrupted databases or thumbnails
- malicious or malformed image files
- drive identity collision
- leaked face embeddings through logs or backups

## Source-file protection

- Open originals read-only.
- Never write sidecars beside source images.
- Never update EXIF or Finder metadata.
- Never move, rename or delete source files.
- Capture modification time before and after processing.
- Halt on detected application-caused modification.
- Prefer read-only mounting when practical, while still supporting ordinary user-mounted volumes.

## Network isolation

The indexing path must make no network calls.

- No remote AI inference
- No model downloads during indexing
- No telemetry
- No analytics
- No remote crash reporting containing archive data
- No automatic upload

Model installation is a separate explicit setup action. Indexing must fail clearly when required local models are absent.

## Encryption

### Face embeddings

Must be encrypted at rest using authenticated encryption.

### Key management

- Generate a random application master key.
- Store or wrap it using macOS Keychain.
- Do not hard-code keys.
- Do not store keys in source control.
- Support key-version rotation.

### Database

Full database encryption is desirable but not required for the first safe vertical slice if FileVault is active and face embeddings are separately encrypted. Document the final decision and threat trade-off.

## Logging

Logs may contain:

- drive number
- relative path when necessary
- stage and structured error code
- timings and counts

Logs must not contain:

- raw embeddings
- decrypted biometric data
- full image content
- OCR text by default
- secrets or key material

Provide a privacy-redacted diagnostics export.

## Malformed files

- Decode in a worker process.
- Enforce memory and time limits.
- Treat crashes as file-level failures where possible.
- Never repeatedly crash-loop on the same file.
- Quarantine only the queue record, not the original file.

## Permissions

Request the minimum macOS permissions necessary. Explain why a folder or volume needs access. Do not request broad access before it is needed.

## Data deletion

The user must be able to:

- remove a drive catalogue while leaving the drive untouched
- remove cached thumbnails
- delete named-person and face-derived data
- reset all generated catalogue data

Every destructive catalogue action requires clear confirmation and must state that originals are not affected.
