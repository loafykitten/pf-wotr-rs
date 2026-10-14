#!/usr/bin/env python3
"""Decode the game's serialized Wwise settings from Data/sharedassets0.assets.

Usage: python3 -I decode_settings.py <Wrath.app>/Contents/Resources/Data/sharedassets0.assets

Finds the AkWwiseInitializationSettings platform object named "Mac" and the
AudioFilePackagesSettings object by scanning MonoBehaviour objects (class 114) of the
Unity 2020.3 serialized file (format 22, no type trees), and decodes their fields in the
declaration order of the decompiled AkCommon*Settings / AudioFilePackagesSettings classes.
Prints the values and whether each decode consumed its object exactly. Read-only.
"""
import struct
import sys

f = open(sys.argv[1], 'rb').read()
p = 0x30
p = f.index(b'\0', p) + 1 + 4  # unity version, platform
assert f[p] == 0, 'type trees present; this decoder expects none'
p += 1
ntypes, = struct.unpack_from('<i', f, p); p += 4
types = []
for _ in range(ntypes):
    cid, = struct.unpack_from('<i', f, p); p += 4 + 1 + 2
    p += 16 + (16 if cid == 114 else 0)
    types.append(cid)
data_offset, = struct.unpack_from('>q', f, 0x20)
nobj, = struct.unpack_from('<i', f, p); p += 4
objects = []
for _ in range(nobj):
    p = (p + 3) & ~3
    pid, start, size, tidx = struct.unpack_from('<qqIi', f, p); p += 24
    if types[tidx] == 114:
        objects.append((pid, data_offset + start, size))


class R:
    def __init__(s, off, size): s.p, s.end = off, off + size
    def al(s): s.p = (s.p + 3) & ~3
    def u(s, fmt):
        v, = struct.unpack_from('<' + fmt, f, s.p); s.p += struct.calcsize(fmt); return v
    def b(s): v = f[s.p]; s.p += 1; s.al(); return bool(v)
    def s(s):
        n = s.u('I'); v = f[s.p:s.p + n].decode(); s.p += n; s.al(); return v
    def header(s):
        s.p += 12; s.b(); s.p += 12; return s.s()


def mac_settings(r):
    out = {}
    out['IgnorePropertyNameList'] = [r.s() for _ in range(r.u('I'))]
    out['GlobalPropertyNameList.count'] = len([r.s() for _ in range(r.u('I'))])
    U = [('m_BasePath', r.s), ('m_StartupLanguage', r.s), ('m_EngineLogging', r.b),
         ('m_MaximumNumberOfPositioningPaths', lambda: r.u('I')), ('m_CommandQueueSize', lambda: r.u('I')),
         ('m_SamplesPerFrame', lambda: r.u('I')), ('Main.m_AudioDeviceShareset', r.s),
         ('Main.m_DeviceID', lambda: r.u('I')), ('Main.m_PanningRule', lambda: r.u('i')),
         ('Main.ChannelConfigType', lambda: r.u('i')), ('Main.ChannelMask', lambda: r.u('i')),
         ('Main.NumberOfChannels', lambda: r.u('I')), ('m_StreamingLookAheadRatio', lambda: r.u('f')),
         ('m_SampleRate', lambda: r.u('I')), ('m_NumberOfRefillsInVoice', lambda: (r.u('H'), r.al())[0]),
         ('SA.m_MaxSoundPropagationDepth', lambda: r.u('I')), ('SA.m_DiffractionFlags', lambda: r.u('i')),
         ('SA.m_MovementThreshold', lambda: r.u('f')), ('SA.m_NumberOfPrimaryRays', lambda: r.u('I')),
         ('SA.m_MaxReflectionOrder', lambda: r.u('I')), ('SA.m_MaxPathLength', lambda: r.u('f')),
         ('SA.m_EnableDiffractionOnReflections', r.b), ('SA.m_EnableDirectPathDiffraction', r.b),
         ('SA.m_EnableTransmission', r.b)]
    A = [('m_IOMemorySize', 'I'), ('m_TargetAutoStreamBufferLengthMs', 'f'), ('m_UseStreamCache', 'b'),
         ('m_MaximumPinnedBytesInCache', 'I'), ('m_EnableGameSyncPreparation', 'b'),
         ('m_ContinuousPlaybackLookAhead', 'I'), ('m_MonitorQueuePoolSize', 'I'),
         ('m_MaximumHardwareTimeoutMs', 'I'), ('m_DebugOutOfRangeCheckEnabled', 'b'),
         ('m_DebugOutOfRangeLimit', 'f'), ('SA.m_DiffractionShadowAttenuationFactor', 'f'),
         ('SA.m_DiffractionShadowDegrees', 'f'), ('m_RenderDuringFocusLoss', 'b'),
         ('m_SoundBankPersistentDataPath', 's'), ('m_UseAsyncOpen', 'b')]
    for k, fn in U:
        out['UserSettings.' + k] = fn()
    for k, t in A:
        out['AdvancedSettings.' + k] = r.b() if t == 'b' else r.s() if t == 's' else r.u(t)
    out['CommsSettings.m_PoolSize'] = r.u('I')
    for k in ('m_DiscoveryBroadcastPort', 'm_CommandPort', 'm_NotificationPort'):
        out['CommsSettings.' + k] = r.u('H')
    r.al()
    out['CommsSettings.m_InitializeSystemComms'] = r.b()
    out['CommsSettings.m_NetworkName'] = r.s()
    out['CommsSettings.m_commSystem'] = r.u('i')
    return out


def packages(r):
    out = {}
    chunks = ['Preload', 'MainGame', 'DLC1', 'DLC2', 'DLC3', 'DLC4', 'DLC5', 'DLC6']
    for field in ('m_PackageMapping', 'm_BankMapping'):
        for i in range(r.u('I')):
            out[f'{field}[{chunks[i] if i < len(chunks) else i}]'] = [r.s() for _ in range(r.u('I'))]
    return out


for pid, off, size in objects:
    r = R(off, size)
    try:
        name = r.header()
    except Exception:
        continue
    decoder = {'Mac': mac_settings, 'AudioFilePackagesSettings': packages}.get(name)
    if not decoder:
        continue
    vals = decoder(r)
    print(f'== {name} (path ID {pid}, {size} bytes, consumed exactly: {r.p == r.end})')
    for k, v in vals.items():
        print(f'{k} = {v!r}')
