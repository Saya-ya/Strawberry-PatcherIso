"""
datafat.py — Canonical FAT logic for Data.bin (Strawberry Panic!, PS2).

IMPORTANT — real format verified against the binary:

  Header (offset 0x00):
    0x00: magic     u32 = 0x7878C800
    0x04: num_files u32 = 27411
    0x08: ???       u32 = 0x8000
    0x0C: data_base u32 = 0x60000

  Table (FAT) at offset 0x8004, num_files records of 12 bytes:
    [id:u32] [size_field:u32] [offset:u32]

  >>> The `offset` of row i DOES correspond to the file with `id` of row i.
  >>> The `size_field` of row i is NOT the size of file i: it is the size
      of the file from the PREVIOUS row. The real (on-disk) size of the file
      from row i is stored in the `size_field` of row i+1.

  This was confirmed by comparing, for the 997 LZ77 scripts, the actual total
  stream (12 + comp_size from the LZ77 header) against size_field: it matches
  997/997 with the NEXT row, never with the row itself.

  The game's loader uses that size (next row) to read the file from CD to RAM.
  That's why, when recompressing a script and writing the new size to the wrong
  row, the game would load fewer bytes than needed and the stream would be
  truncated -> corruption (the root cause of the historical bug).

Usage rules:
  - To READ the real file size of a row: use `entry['size']`.
  - To WRITE the new real size: write to the size_field of the NEXT row
    (see `size_field_write_offset`).
"""

import struct

FAT_OFFSET = 0x8004
NUM_ENTRIES = 27411
ENTRY_SIZE = 12 


def read_fat_raw(source):
    """Returns raw FAT bytes from a path or from Data.bin bytes."""
    if isinstance(source, (bytes, bytearray)):
        return bytes(source[FAT_OFFSET:FAT_OFFSET + NUM_ENTRIES * ENTRY_SIZE])
    with open(source, 'rb') as f:
        f.seek(FAT_OFFSET)
        return f.read(NUM_ENTRIES * ENTRY_SIZE)


def parse_entries(fat_raw):
    """
    Returns a list of dicts, one per FAT row, with:
      row        : row index
      id         : file id
      size_field : raw value of the field (= size of the file from the previous row)
      off        : offset in Data.bin of THIS row's file
      size       : REAL size of this row's file (= size_field of row i+1)
      is_file    : True if off > 0 (rows with off == 0 are sentinel/non-file)
    """
    rows = []
    for i in range(NUM_ENTRIES):
        fid, size_field, off = struct.unpack_from('<III', fat_raw, i * ENTRY_SIZE)
        rows.append({'row': i, 'id': fid, 'size_field': size_field, 'off': off})
    for i in range(NUM_ENTRIES):
        nxt = rows[i + 1]['size_field'] if i + 1 < NUM_ENTRIES else rows[i]['size_field']
        rows[i]['size'] = nxt
        rows[i]['is_file'] = rows[i]['off'] > 0
    return rows


def read_entries(source):
    """Shortcut: reads and parses FAT from path or bytes."""
    return parse_entries(read_fat_raw(source))


def find_row(rows, fid):
    """Finds the row (dict) of a real file ID (off > 0). None if does not exist."""
    for r in rows:
        if r['id'] == fid and r['off'] > 0:
            return r
    return None


def slot_capacity(rows, row):
    """
    Physical capacity of the slot = distance to the next offset on disk.
    It is >= real size (includes 0x800 alignment padding). Useful to know
    whether a recompressed stream fits without overwriting the neighbor file.
    """
    offs = sorted(r['off'] for r in rows if r['off'] > 0)
    o = row['off']
    idx = offs.index(o)
    if idx + 1 < len(offs):
        return offs[idx + 1] - o
    return row['size']


def size_field_write_offset(row):
    """
    Absolute offset (in Data.bin) where the real file size of `row`
    must be WRITTEN: the size_field of the NEXT row.
    """
    return FAT_OFFSET + (row['row'] + 1) * ENTRY_SIZE + 4
