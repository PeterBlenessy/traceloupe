# tools

## `make_fixture_backup.py`

Generates a tiny (~48 KB) but cryptographically valid **encrypted** iOS backup
for tests and the iLEAPP spike, so we never need a real multi-GB backup in the
repo or CI. Reproduces the iTunes-backup format (keybag, two-stage PBKDF2 KEK,
AES-key-wrapped class/manifest/file keys, AES-CBC file blobs) with a seeded
`sms.db`.

```sh
pip install cryptography
python tools/make_fixture_backup.py /tmp/fixture-backup --password salvage-test
```

The fixture's crypto is independently validated by the third-party `iOSbackup`
library and its parsing by iLEAPP — see `docs/spike-ileapp.md`. Extend
`seed_files()` to add more artifact classes (each needs the schema the
corresponding iLEAPP module queries).
