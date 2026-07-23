# Bundled basic dictionary

`mozc-basic.tsv` is a deterministic, reduced extract of the Mozc open-source
dictionary at revision `3f235b4eb6fcff7d14ef5f0fb8ee56de7ee4c732`.

The extract keeps entries whose Mozc word cost is at most 8500, keeps the
literal reading candidate for every included reading, and keeps the lowest cost
for duplicate reading/surface/left-ID/right-ID tuples. The result contains
1,085,464 entries in this format:

```text
reading<TAB>surface<TAB>left_id<TAB>right_id<TAB>word_cost
```

The 8500 threshold was chosen on AJIMEE-Bench: accuracy stops improving above
it (the full, unfiltered dictionary scores identically), while 7500 loses about
two points of top-1 accuracy.

The TSV is the source of truth; `build.rs` compiles it at build time into a
zero-copy binary form (an FST over readings, a per-reading entry table, and a
deduplicated surface pool, about 29 MB total) that the converter embeds with
`include_bytes!`. Nothing parses the TSV at runtime.

`mozc-connection.bin` contains Mozc's 2,672 by 2,672 connection matrix stored
exactly (16-bit costs, format `UCN2`): each row is its most frequent value plus
sparse exceptions. The resulting matrix is about 3.6 MB instead of the 35 MB
source text.

Regenerate it from the pinned upstream revision with:

```sh
scripts/update-mozc-basic-dictionary.sh
```

The source dictionary, connection matrix, and these derived extracts are covered by the notices in
[`MOZC_DICTIONARY_LICENSE.txt`](./MOZC_DICTIONARY_LICENSE.txt).
