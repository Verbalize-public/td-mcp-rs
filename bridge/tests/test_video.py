"""Bounded MOV verification and partial/unfinalized-file rejection."""
import struct
import sys
from pathlib import Path

import pytest
sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from tdmcp_bridge.video import verify_movie, boxes


def atom(kind, body):
    return struct.pack('>I4s', len(body) + 8, kind) + body


def movie(frames=7):
    entry = bytearray(40)
    struct.pack_into('>I4s', entry, 0, 40, b'rle ')
    struct.pack_into('>HH', entry, 32, 64, 64)
    table = atom(b'stsz', struct.pack('>III', 0, 1, frames))
    table += atom(b'stsd', struct.pack('>II', 0, 1) + entry)
    mdhd = bytes(12) + struct.pack('>II', 60, frames)
    media = atom(b'mdhd', mdhd) + atom(b'hdlr', bytes(8) + b'vide')
    media += atom(b'minf', atom(b'stbl', table))
    return atom(b'mdat', bytes(frames)) + atom(b'moov', atom(b'trak', atom(b'mdia', media)))


def test_verifies_exact_count_dimensions_fps(tmp_path):
    path = tmp_path / 'recording.mov'
    path.write_bytes(movie())
    assert verify_movie(path, 7, 64, 64, 60)['frames'] == 7
    for expected in ((6, 64, 64, 60), (7, 32, 64, 60), (7, 64, 64, 30)):
        with pytest.raises(ValueError, match='verification failed'):
            verify_movie(path, *expected)


@pytest.mark.parametrize('payload', [b'', b'garbage', atom(b'mdat', b'data'), movie()[:-5]])
def test_unfinalized_truncated_or_empty_movie_rejected(tmp_path, payload):
    path = tmp_path / 'recording.mov'
    path.write_bytes(payload)
    with pytest.raises((ValueError, struct.error)):
        verify_movie(path, 7, 64, 64, 60)


def test_atom_bounds():
    for value in (b'1', struct.pack('>I4s', 4, b'bad!'), struct.pack('>I4s', 100, b'bad!')):
        with pytest.raises(ValueError):
            list(boxes(value))
