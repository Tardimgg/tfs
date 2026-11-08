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

## Revocations

The offline root CA operator signs the exact bytes of a JSON document such as:

```json
{"sequence":1,"issuer_spki_sha256":"0123456789ABCDEF0123456789ABCDEF0123456789ABCDEF0123456789ABCDEF","revoked_serials":["01AF","02BC"]}
```

The signed message is the byte concatenation:

```text
tfs-dtls-revocations\0 || exact JSON bytes
```

`sequence` must be positive and increase by exactly one for each administrative update. `issuer_spki_sha256` is the SHA-256 digest of the root CA SubjectPublicKeyInfo. Certificate serials use one to twenty bytes of hexadecimal data; leading zero bytes are significant.

The signature algorithm is RSA-PSS with SHA-256, MGF1 SHA-256, and exactly 32 bytes of salt. Other padding, hashes, or salt lengths are rejected.

Provide the first signed document and its detached signature as the bootstrap files. The signature file accepts raw 512-byte RSA-4096 signatures, hexadecimal, or standard base64 (with or without padding). At startup the daemon compares the bundled artifact with its persisted state, rejects equal-sequence conflicts, selects the higher valid sequence, and persists it before starting application services.

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
  -H "x-tfs-revocation-signature: $(base64 -w0 revocations.sig)" \
  https://NODE/security/dtls/revocations
```

The daemon verifies the detached root-CA RSA-PSS signature, wraps the exact JSON and variable-length signature bytes in canonical protobuf plus zstd (level 19), and publishes one reserved BEP44 value of at most 1000 bytes. RSA-4096 signatures alone occupy 512 bytes; this format is intended for a few revocations, not a large CRL. Tests cover five random 20-byte serials, but the actual encoded size is always enforced. Oversized updates are rejected, not split or truncated. The outer BEP44 sequence must equal the signed JSON sequence. Invalid or noncanonical values for the reserved target are rejected by every storage node. Administrative replay, sequence gaps, and equal-sequence conflicts return HTTP 409. New state is persisted before it is applied.

During startup, the canonical local artifact is installed in protected DHT storage before synchronization. Until synchronization completes, nodes admit only GET and signed mutable PUT requests for the reserved revocation target; all ordinary DHT requests remain blocked and these control-plane exchanges are not added to routing tables. This allows a cluster-wide cold restart without serving application data under stale revocation state.

Serving is activated only when the actor confirms that protected local storage exactly matches the applied artifact; pre-activation DTLS sessions are then reset. Startup retries transient network failures and newer-artifact races up to eight attempts, with backoff from 250 ms to 4 s, while application serving remains closed. Invalid signatures, equivocation, and persistence failures are not transient retries.

A signed sequence conflict, or a failure to persist or enforce an observed newer signed artifact, latches a fatal security failure. Ordinary application DHT reads, writes, background requests, queued sends, and in-flight results are rejected; the DHT HTTP API returns 503. Protected revocation recovery exchanges remain available. Applying a newer artifact does not clear the latch: operator recovery and a process restart are required before ordinary operations resume. Snapshots are file-synced and atomically renamed, then their parent directory is synced, including for bare relative paths; newly created directory ancestors are also synced.

Invalid or noncanonical remote candidates are discarded individually, so they cannot suppress other verified remote or local artifacts. Equivocation between accepted root-signed artifacts remains fatal. If persistence or post-persistence snapshot enforcement fails, registry advancement becomes terminal until restart: no later synchronization or publication may rewrite the stale in-memory sequence over a possibly newer on-disk snapshot. Restart verifies and selects the highest available persisted/bootstrap sequence before resuming. This does not claim durability when the filesystem reports an fsync failure.

## Capturing connection failures

TFS uses `env_logger`; mainline's `tracing` events are forwarded through tracing's `log` feature without installing a tracing subscriber. Defaults are `info` for the application and `warn` for mainline. `RUST_LOG` overrides these defaults. Connection warnings include the peer address, direction, stage, error, and configured timeout for handshakes, without dumping packets, certificates, or keys. Certificate verification performed within the DTLS handshake is reported at the `handshake` stage; post-handshake validation uses `certificate_validation`. Inbound peer ports may be ephemeral outbound source ports, not the other daemon's listening port.

For focused diagnostics, use:

```sh
RUST_LOG='info,mainline=warn,mainline::rpc::transport=debug'
```

The transport debug override includes serving-gate retirement/reset and worker-shutdown registration cancellation. These are lifecycle events, not authentication warnings. Avoid global `trace` or DTLS-library packet tracing when collecting logs. `NoClosestNodes` means no storage candidates were found, not proof of a certificate failure; correlate it with preceding transport warnings. `Too few keepers` includes self, known keeper count before excluding self, self-registration status, other keeper count, and the configured replication factor.

To update two existing localhost installations:

1. From the repository root, build once: `cargo build --locked --release --manifest-path tfs/Cargo.toml --target-dir tfs/target`.
2. Stop both old daemon processes using their usual supervisor or Ctrl-C, and wait for them to exit. Copy that same `tfs/target/release/tfs` executable to both installation executable paths. For example, after replacing the placeholder directories:

```sh
install -m 755 tfs/target/release/tfs /absolute/node-a/tfs
install -m 755 tfs/target/release/tfs /absolute/node-b/tfs
sha256sum tfs/target/release/tfs /absolute/node-a/tfs /absolute/node-b/tfs
```

3. Check that all three hashes match. Preserve each node's own TLS/DTLS identity, existing configuration, and persisted state; copy only the executable.
4. Restart each daemon from its existing working directory, preserving its existing environment and self/bootstrap arguments. For example, for nodes listening on 8080 and 8081, run these in separate terminals in the respective node directories:

```sh
# Node A's existing working directory:
RUST_LOG='info,mainline=warn,mainline::rpc::transport=debug' \
  ./tfs 127.0.0.1:8080 127.0.0.1:8081 > daemon-diagnostics.log 2>&1
```

```sh
# Node B's existing working directory:
RUST_LOG='info,mainline=warn,mainline::rpc::transport=debug' \
  ./tfs 127.0.0.1:8081 127.0.0.1:8080 > daemon-diagnostics.log 2>&1
```

For supervisor-managed daemons, set the same `RUST_LOG` in both service configurations, ensure their executable paths point to the updated copies, and restart both services. Setting it only in an interactive shell does not change an already running service. Capture both stdout and stderr from startup through the first repeated failures, retaining timestamps and peer/stage/error fields. Do not include private keys or full environment dumps in a report.
