# Bundled basic dictionary

`mozc-basic.tsv` is a deterministic, reduced extract of the Mozc open-source
dictionary at revision `3f235b4eb6fcff7d14ef5f0fb8ee56de7ee4c732`.

The extract keeps entries whose Mozc word cost is at most 5500, keeps the
literal reading candidate for every included reading, and keeps the lowest cost
for duplicate reading/surface/left-ID/right-ID tuples. The result contains
170,229 entries in this format:

```text
reading<TAB>surface<TAB>left_id<TAB>right_id<TAB>word_cost
```

`mozc-connection.bin` contains Mozc's 2,672 by 2,672 connection matrix. Costs
are quantized to one byte at a resolution of 64, then each row is stored as its
mode plus sparse exceptions. The resulting matrix is about 2.7 MB instead of
the 35 MB source text.

Regenerate it from the pinned upstream revision with:

```sh
scripts/update-mozc-basic-dictionary.sh
```

The source dictionary, connection matrix, and these derived extracts are covered by the notices in
[`MOZC_DICTIONARY_LICENSE.txt`](./MOZC_DICTIONARY_LICENSE.txt).
