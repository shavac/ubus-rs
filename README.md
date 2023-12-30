ubus (Rust)
===========

**Work in progress**

This is a pure Rust library for implementing [OpenWRT ubus](https://openwrt.org/docs/techref/ubus) clients.

Goals
-----

* Minimal bloat
* Few dependencies
* Zero allocations inside main code
* `no_std` where possible
* Don't panic!

Supported
---------

* Unix-Domain-Socket + Type-Length-Value protocol support
* `blob` TLV format support
* High-level abstraction for `lookup` command
* High level abstraction for `call` command
* JSON support

TODO
----

* High level abstraction for `subscribe`/`unsubscribe` commands
* HTTP(S) + JSON protocol support
