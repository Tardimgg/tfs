# DHT DTLS configuration

KRPC uses DTLS 1.2 with mutual certificate authentication. Plaintext KRPC and the former plaintext HTTP listener are not available.

## Certificates

Each node needs a separate EC P-256 certificate and private key. The certificate must be issued directly by the existing RSA-4096 root CA, contain both `clientAuth` and `serverAuth` extended key usages, and contain the shared DNS SAN `dht.tfs.internal` unless `DtlsConfig::with_server_name` is used by an embedding application. The root CA path must contain exactly one PEM certificate with an RSA-4096 public key.

Configure these paths:

| Environment variable | Default |
| --- | --- |
| `DTLS_NODE_CERT_PATH` | `tls/dtls-node.crt` |
| `DTLS_NODE_KEY_PATH` | `tls/dtls-node.key` |
| `DTLS_ROOT_CA_PATH` | `tls/dtls-root-ca.pem` |
| `DTLS_REVOCATION_BOOTSTRAP_JSON_PATH` | `tls/dtls-revocations.json` |
| `DTLS_REVOCATION_BOOTSTRAP_SIGNATURE_PATH` | `tls/dtls-revocations.sig` |
| `DTLS_REVOCATION_STATE_PATH` | `root/security/dtls-revocations.snapshot` |
| `DTLS_REVOCATION_POLL_INTERVAL_SECS` | `30` |

The root CA certificate supplies the revocation verification key. There is no separate revocation authority key or authority-key path. Keep the existing root CA private key offline; node processes never load it. Ed25519 revocation signatures are not supported. The outer BEP44 protocol still uses its Ed25519 signature, which is not the authorization for revocations.

## Privileged HTTP roles

Raw `/dht/{key}` GET and POST require an access-token bearer JWT and the `RawDhtAccess` role. HTTPS publication to `/security/dtls/revocations` requires `RevocationRegistryPublisher` **and** a valid root-CA signature on the submitted document. The role restricts HTTP admission; it does not authorize the document itself. The raw POST endpoint still rejects the reserved revocation key.

Access tokens use the same `Authorization: Bearer` header. An ordinary session token has the user's stored roles. An extended access token is a PS256 JWT signed offline by the root from `DTLS_ROOT_CA_PATH`. Its payload contains `uid` (an existing user's numeric ID), `additional_roles` (a list of role names), and `exp` (Unix time in seconds). An extended token adds its signed roles to that user's rights until `exp`. Use a short expiry. Extended tokens cannot refresh a session. A token is valid on any deployment that trusts the same DTLS root.

To set up the first administrator, issue an extended token with `roles:management` for an existing operator UID. Submit `POST /user/{uid}/roles` with that bearer token and the roles to add to the user at the UID in the URL, for example:

```json
{"roles":["roles:management","RawDhtAccess"]}
```

The endpoint returns `204` after confirming the additive grant. A user with `roles:management` can assign any role, including `roles:management` to their own UID. Repeating a confirmed grant is a no-op. Roles already stored in `UserInfo` stay in effect; records without a roles field have no stored roles. The startup roles file is no longer used. Token expiry ends the temporary roles but does not undo grants that were saved.

The previous spelling `roles:grant` remains accepted on input, including in stored user records and older tokens. Newly serialized values use `roles:management`.

Only the observed DHT view is confirmed; this is not a cluster-wide transaction. Send role grants to a node that stores the target user's exact committed profile version. A request that depends on a stored role also needs that version of the caller's profile locally. Another node's latest profile is not proof of the committed version. A matching additional role in an extended token does not need a stored role lookup. Raw DHT access and the DHT peers are trusted with cluster data, including the ordinary JWT signing configuration. File access still uses the UID and the existing ReBAC checks.

Provision a strong, private JWT signing secret as a string at the existing DHT key `/config/secret_signing_secret` before using ordinary sessions. Existing deployments with an explicitly provisioned key can keep it. There is no built-in default; missing, unreadable, empty, or whitespace-only values make ordinary token issuance and verification unavailable. HTTP requests that need the key return 503, and registration does not write user state without it. The daemon can start without the key, and routes that allow anonymous access still accept requests without an Authorization header. Valid keys use the existing dynamic configuration cache TTL.

## Revocations

The offline root CA operator signs the exact bytes of a JSON document such as:

```json
{"sequence":1,"issuer_spki_sha256":"0123456789ABCDEF0123456789ABCDEF0123456789ABCDEF0123456789ABCDEF","revoked_serials":["01AF","02BC"]}
```

The signed message is the byte concatenation:

```text
tfs-dtls-revocations\0 || exact JSON bytes
```

`sequence` must be positive and increase by exactly one for each administrative update. `issuer_spki_sha256` is the SHA-256 digest of the root CA SubjectPublicKeyInfo. Certificate serials use one to twenty bytes of hexadecimal data; leading zero bytes are significant. Sign with RSA-PSS, SHA-256, MGF1 SHA-256, and exactly 32 bytes of salt; other padding, hashes, or salt lengths are rejected.

Before launching each node, ensure the latest signed revocation snapshot is on that node's disk. If its persisted snapshot is missing or out of date, provide the latest JSON document and its detached signature at the configured bootstrap paths. The signature file accepts raw 512-byte RSA-4096 signatures, hexadecimal, or standard base64 (with or without padding). At startup the daemon verifies the local bootstrap and persisted snapshots, rejects equal-sequence conflicts, selects the highest valid local sequence, and persists it before starting application services.

### Offline signing with OpenSSL

Run these commands on the offline signing machine, using the existing `dtls-root-ca.key` and matching `dtls-root-ca.pem`. Do not generate a separate signing key. These examples use literal paths and no environment variables.

Extract the public key and print the issuer SPKI SHA-256 digest:

```sh
openssl x509 -in dtls-root-ca.pem -pubkey -noout -out dtls-root-ca.pub.pem
openssl pkey -pubin -in dtls-root-ca.pub.pem -outform DER -out dtls-root-ca.spki.der
openssl dgst -sha256 dtls-root-ca.spki.der
```

Put that 64-character hexadecimal digest in `issuer_spki_sha256` in `revocations.json`. For the initial document use sequence 1 and an empty `revoked_serials` array if no certificates are revoked. For each update increment the sequence by one and include the complete desired revoked-serial set. Then construct the domain-prefixed message and sign it:

```sh
printf 'tfs-dtls-revocations\000' > revocations.payload
cat revocations.json >> revocations.payload
openssl dgst -sha256 -sign dtls-root-ca.key \
  -sigopt rsa_padding_mode:pss \
  -sigopt rsa_mgf1_md:sha256 \
  -sigopt rsa_pss_saltlen:32 \
  -out revocations.sig revocations.payload
openssl dgst -sha256 -verify dtls-root-ca.pub.pem \
  -sigopt rsa_padding_mode:pss \
  -sigopt rsa_mgf1_md:sha256 \
  -sigopt rsa_pss_saltlen:32 \
  -signature revocations.sig revocations.payload
```

Verification must report `Verified OK`. Transfer only the exact JSON and detached signature to the publishing machine (or the bootstrap paths); do not transfer the CA private key. Whitespace and the final newline in the JSON are signed too. RSA-PSS is randomized: retain and distribute the same signature file for a sequence, rather than signing the same JSON again. Different JSON or signature bytes at an already observed sequence are a conflict.

Publish an update over HTTPS only:

```sh
curl --fail --data-binary @revocations.json \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer ${TFS_ACCESS_TOKEN}" \
  -H "x-tfs-revocation-signature: $(base64 -w0 revocations.sig)" \
  https://NODE/security/dtls/revocations
```

The daemon verifies the detached root-CA signature and publishes one reserved BEP44 value of at most 1000 bytes, with an outer sequence matching the signed JSON sequence. The format is for a few revocations; oversized updates are rejected rather than split or truncated. Storage nodes reject invalid or noncanonical values for the reserved target, and polling discards such candidates individually so they cannot suppress verified artifacts. Administrative replay, sequence gaps, and equal-sequence conflicts return HTTP 409. Before applying a snapshot, the daemon syncs the file, atomically renames it, and syncs its parent directory (even for bare relative paths) and any newly created ancestors.

At startup, DHT serving opens only after protected local storage matches the applied snapshot. DTLS handshakes and outbound bootstrap probes can start earlier, but inbound DHT traffic is discarded and revocation exchange is disabled until serving opens and pre-activation sessions are reset. Startup does not check cluster freshness: a disconnected node can serve with an older valid local snapshot. Normal DHT polling and HTTPS publication resume once serving opens.

Conflicting root-signed artifacts at the same sequence, or failure to persist or enforce an observed newer signed artifact, latch a fatal security failure. All DHT traffic stops, including polling and revocation exchange; reads, writes, background requests, queued sends, and in-flight results are rejected, and the DHT HTTP API returns 503. Synchronization and publication cannot overwrite a potentially newer disk snapshot with stale in-memory state. A filesystem sync failure means durability is not assured.

To recover, correct the cause, manually install the corrected, up-to-date signed JSON and its matching detached signature at the configured local bootstrap paths, then restart the daemon. Restart rechecks local files and selects the highest valid sequence as described above. The latch does not clear on its own, and HTTPS publication cannot recover the node.

## Capturing connection failures

Logging defaults to `info` for the application and `warn` for mainline. For focused diagnostics, use the `RUST_LOG` override below instead of global `trace` or DTLS packet tracing. Capture stdout and stderr from startup through the first repeated failures, retaining timestamps, peer address, direction, stage, error, and handshake timeout. Inbound peer ports may be ephemeral. DTLS handshake certificate checks appear under `handshake`; later checks use `certificate_validation`. `NoClosestNodes` means no storage candidates were found; check earlier transport warnings before attributing it to certificates. Omit private keys and full environment dumps from reports.

To roll out to two existing localhost installations:

1. From the repository root, build once: `cargo build --locked --release --manifest-path tfs/Cargo.toml --target-dir tfs/target`.
2. Stop both daemons and wait for exit. Install the same `tfs/target/release/tfs` at each executable path using `install -m 755`; use `sha256sum` to confirm both copies match the build. Preserve each node's TLS/DTLS identity, configuration, and persisted state.
3. Before restarting either node, check that both have the latest signed snapshot on disk. Where needed, install the same signed JSON and detached signature at the configured bootstrap paths. Keep each node's persisted state and private keys local.
4. Restart each node from its existing working directory with its existing environment and self/bootstrap arguments. For supervised daemons, point each service at the updated executable and set `RUST_LOG` in the service configuration, not just in your shell. For a manual restart, use this example on the first node and swap the ports for the second:

   ```sh
   RUST_LOG='info,mainline=warn,mainline::rpc::transport=debug' \
     ./tfs 127.0.0.1:8080 127.0.0.1:8081 > daemon-diagnostics.log 2>&1
   ```
