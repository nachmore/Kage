# Folder sync envelope, version 1 golden files

Files written by `sync::envelope` and `sync::crypto` in the version 1
format (docs/design/SYNC.md §5.4, §11.3). The test
`sync::envelope::tests::golden_files_of_version_1_open` opens every one of
them. They must keep opening in every later build: never edit or
regenerate them. A new format version adds its own files next to these.

All inputs are synthetic and fixed in that test module (`golden`):

- data key: bytes `0xa0 ^ i` for `i` in `0..32`;
- folder id `5qg2vbn8h1xk0d3m`, device `k7m2p9xqa3rt5vwe`, epoch 1;
- `head-000000000042.kage`: a head naming one `cfg` shard;
- the two `<32 hex>.kage` files: that shard, and a blob of about 72 KiB of
  repeated text, each named by its content;
- `e0001-k7m2p9xqa3rt5vwe-0001.json`: the data key wrapped with the
  passphrase `ABCDE-FGHJK-MNPQR-STVWX-YZ012` at the real Argon2id cost
  (64 MiB, t = 3, p = 1).

The nonces are fixed so the files are reproducible. A real writer never
reuses a nonce.
