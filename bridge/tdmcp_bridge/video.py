"""Bounded verification of the finalized, single-video-track qtrle MOV.

Checks sample tables/container metadata, not decoder correctness. Live acceptance
also decodes first/last images with ffmpeg. No TD API access in this module.
"""
import os
import struct


def boxes(data):
    offset = 0
    while offset < len(data):
        if len(data) - offset < 8:
            raise ValueError('Truncated MOV atom header')
        size, kind = struct.unpack_from('>I4s', data, offset)
        header = 8
        if size == 1:
            if len(data) - offset < 16:
                raise ValueError('Truncated extended MOV atom')
            size = struct.unpack_from('>Q', data, offset + 8)[0]
            header = 16
        if size == 0:
            size = len(data) - offset
        if size < header or offset + size > len(data):
            raise ValueError('Invalid MOV atom size')
        yield kind, data[offset + header:offset + size]
        offset += size


def one(data, kind):
    found = [payload for name, payload in boxes(data) if name == kind]
    if len(found) != 1:
        raise ValueError(f'Expected one MOV {kind!r} atom')
    return found[0]


def read_moov(path):
    total = os.path.getsize(path)
    if not 0 < total <= 512 * 1024 * 1024:
        raise ValueError('Movie size outside 1..512 MiB')
    moov = None
    with open(path, 'rb') as stream:
        while stream.tell() < total:
            start = stream.tell()
            header = stream.read(8)
            if len(header) != 8:
                raise ValueError('Truncated MOV header')
            size, kind = struct.unpack('>I4s', header)
            width = 8
            if size == 1:
                extended = stream.read(8)
                if len(extended) != 8:
                    raise ValueError('Truncated extended MOV header')
                size = struct.unpack('>Q', extended)[0]
                width = 16
            if size == 0:
                size = total - start
            if size < width or start + size > total:
                raise ValueError('Invalid MOV file atom size')
            if kind == b'moov':
                if moov is not None or size > 16 * 1024 * 1024:
                    raise ValueError('Duplicate/oversized MOV metadata')
                moov = stream.read(size - width)
            stream.seek(start + size)
    if moov is None:
        raise ValueError('Movie not finalized: no moov atom')
    return moov, total


def verify_movie(path, expected_frames, width, height, rate):
    moov, total = read_moov(path)
    track = one(moov, b'trak')
    media = one(track, b'mdia')
    if one(media, b'hdlr')[8:12] != b'vide':
        raise ValueError('Movie is not a single video track')
    table = one(one(media, b'minf'), b'stbl')
    sizes = one(table, b'stsz')
    _, frames = struct.unpack_from('>II', sizes, 4)
    description = one(table, b'stsd')
    entry = description[8:]
    if entry[4:8] != b'rle ':
        raise ValueError('Unexpected codec; only qtrle is verified')
    actual_width, actual_height = struct.unpack_from('>HH', entry, 32)
    mdhd = one(media, b'mdhd')
    if mdhd[0] == 0:
        timescale, duration = struct.unpack_from('>II', mdhd, 12)
    elif mdhd[0] == 1:
        timescale, duration = struct.unpack_from('>IQ', mdhd, 20)
    else:
        raise ValueError('Unsupported MOV media header version')
    if (frames != expected_frames or (actual_width, actual_height) != (width, height)
            or not duration or not timescale
            or abs(frames * timescale / duration - rate) > 0.01):
        raise ValueError(f'Movie verification failed: {frames} frames, {actual_width}x{actual_height}, '
                         f'timescale={timescale}, duration={duration}; expected '
                         f'{expected_frames} frames, {width}x{height}, {rate} FPS')
    return {'frames': frames, 'width': width, 'height': height, 'fps': rate,
            'durationSeconds': duration / timescale, 'codec': 'qtrle', 'bytes': total,
            'verification': 'container-sample-table'}
