# CID to Unicode tables

The worker reads text from CJK PDF fonts that have no ToUnicode map with
these tables (docs/design/EXTENSION_FILE_TEXT.md §6.2 step 7). Each file
maps the CIDs of one Adobe character collection to Unicode, in the
compact format described in `../cid_tables.rs`.

| File | Source | Source version |
|---|---|---|
| `adobe-japan1.bin` | `Adobe-Japan1-UCS2` (Adobe-Japan1-7) | 10.002 |
| `adobe-korea1.bin` | `Adobe-Korea1-UCS2` (Adobe-Korea1-2) | 5.005 |
| `adobe-gb1.bin` | `Adobe-GB1-UCS2` (Adobe-GB1-5) | 9.000 |
| `adobe-cns1.bin` | `Adobe-CNS1-UCS2` (Adobe-CNS1-7) | 15.000 |

The sources are the `pdf2unicode/` CMaps of Adobe's
[mapping-resources-pdf](https://github.com/adobe-type-tools/mapping-resources-pdf)
repository. Copyright 1990-2023 Adobe. They are distributed under the
BSD-3-Clause licence in `LICENSE.txt`, which must ship with Kage's other
licence notices.

The tables differ from the sources in two ways: variation selectors are
dropped (a CID for a glyph variant maps to its base character), and
`U+FFFD` entries are left out.

To rebuild the tables after Adobe updates the sources:

```sh
git clone https://github.com/adobe-type-tools/mapping-resources-pdf
KAGE_ADOBE_CMAPS=mapping-resources-pdf/pdf2unicode \
  cargo test -p kage-file-text --lib regenerate_adobe_tables -- --ignored
```

With `KAGE_ADOBE_CMAPS` set, the `the_built_in_tables_match_adobes_files`
test checks that the files here match the sources.
