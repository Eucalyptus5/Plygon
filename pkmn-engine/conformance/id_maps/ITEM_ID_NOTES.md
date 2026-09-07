# Item ID Mapping

## Confirmed: Items use Showdown `spritenum` as the engine index

Source: `pkmn-engine/codegen.py` line 741:
```python
# Use spritenum as the index (matches the old hand-crafted gen_items.rs)
sn = extract_field(block, "spritenum")
```

The `GEN_ITEMS` array is indexed by `spritenum`, NOT by `num`.

## Mapping
- `item_map.json`: `{ showdown_key: spritenum }`
- To look up an item in the engine: `GEN_ITEMS[spritenum]`
- Items with `spritenum <= 0` are skipped
- Items marked `isNonstandard` are skipped in codegen
- Items with no engine-relevant flags produce `ItemData::NONE` (all zeros)
