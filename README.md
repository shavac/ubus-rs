# ubus (Rust)

A pure Rust library for implementing [OpenWrt ubus](https://openwrt.org/docs/techref/ubus) clients and services.

## Goals

* **Minimal bloat & low memory overhead**: Compact `Connection` state, reusable I/O buffer with automatic 4 KiB high-water-mark reclamation, and zero-copy TLV parsing suited for embedded Linux targets.
* **Minimal dependencies**: Built on `serde`, `thiserror`, and optional `serde_json`, with no external endianness crate.
* **Safe Rust**: Zero `unsafe` code with cross-endian safe wire encoding/decoding.
* **Fallible APIs**: `Result`-based error handling without unexpected panics on malformed wire data or I/O errors.

## Supported Features

* **Unix Domain Socket & custom I/O**: `Connection::connect`, non-blocking mode, `AsRawFd`, `reconnect` / `reconnect_and_restore`, and a pluggable `IO` trait.
* **`blob` / `blobmsg` TLV codec**: Zero-copy parsing via `Blob` and `BlobIter`, plus `BlobBuilder` and `BlobMsgBuilder`.
* **Serde integration**: Direct serialization/deserialization without JSON overhead (`to_blob`, `to_blob_into`, `from_blob`, `BlobTable`).
* **Client RPC**: `lookup`, `lookup_id`, `invoke`, `invoke_with_timeout`, `invoke_no_reply`, `invoke_typed`, `invoke_serde`, `ping`, and `wait_for`.
* **JSON helpers** (`json` feature, enabled by default): `call`, `call_pretty`, `call_typed`, `lookup_object_json`, `json_to_blob`, and `args_from_json`.
* **Server-side object hosting**: `add_object`, `remove_object`, `dispatch_one`, `serve`, `reply_data`, and `reply_status`.
* **Publish / subscribe notifications**: `register_subscriber`, `subscribe`, `unsubscribe`, `notify` / `notify_no_reply` (raw, Serde, and JSON variants), plus subscriber `ACTIVE` and object removal callbacks via `dispatch_event` / `serve_events`.
* **System events & monitoring**: `send_event` (raw, Serde, and JSON variants), `listen_events`, and `monitor`.
* **Multiplexing & timeouts**: Per-request deadlines and bidirectional message handling with a bounded pending-message queue.

## Cargo Features

* `default = ["json"]` — Enables `serde_json` and JSON convenience APIs (`call`, `call_pretty`, `call_typed`, `lookup_object_json`, `json_to_blob`, `args_from_json`, `*_json`).
* Disable JSON support to minimize binary size:

```bash
cargo build --no-default-features
```

## Examples

```bash
# List objects exposed on /var/run/ubus/ubus.sock (works without `json` feature)
cargo run --no-default-features --example lookup

# Call a method using JSON arguments and print pretty JSON (requires `json` feature)
cargo run --example invoke
cargo run --example ubuscall -- system board
```
