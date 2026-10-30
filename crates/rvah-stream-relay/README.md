# RVAH transient stream relay core

This crate is the bounded, memory-only center of a future stream proxy. It does
not store video in Veilid DHT records and does not open a public HTTP service.
The service layer must authorize an upstream, segment it, and carry the returned
`EncryptedFrame` values over a confidential peer/private-route transport.

Typical service flow:

1. Generate a `SessionId` and `SessionKey`, enroll only authorized clients, and
   call `RelayBuffer::register_session` with every client whose receipt is
   required.
2. Pass each authorized upstream segment to `RelayBuffer::ingest` and send the
   slice returned by `RelayBuffer::frames`.
3. The client calls `receive_segment`, commits the verified bytes to its player,
   then returns the generated `DeliveryReceipt`.
4. Call `RelayBuffer::acknowledge`. It returns `true` when the final required
   receipt caused the buffered ciphertext to be discarded.
5. Call `prune_expired` on a timer. Capacity limits also evict the oldest
   segment automatically and report every eviction to the service.

`RelayLimits::default()` permits 64 MiB, 32 segments, and 30 seconds. Production
values should be explicit and included in resource accounting. The caller must
disable or protect swap and crash dumps if it ever handles sensitive plaintext;
the relay core zeroizes keys and failed reassembly buffers but cannot control
copies held by an upstream decoder, allocator, transport, or operating system.
