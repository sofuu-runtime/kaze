#!/usr/bin/env python3
"""mavlink_layout.py -- print the MAVLink 2 wire layout this bridge relies on.

The C codec uses fixed payload offsets, so those offsets have to come from the
dialect rather than from memory. This prints, for every message the bridge
speaks: id, CRC_EXTRA, the base payload length, and each field's offset in wire
order -- base fields first, then MAVLink 2 extension fields, which is why (for
example) COMMAND_ACK.command is at 0 and its result_param2 is at 4.

  python3 scripts/sitl/mavlink_layout.py

The byte-level truth is still pymavlink's own encoder;
scripts/sitl/mavlink_xcheck.py compares frames in both directions.
"""

from __future__ import annotations

import sys

from pymavlink.dialects.v20 import ardupilotmega as mavlink

SIZES = {
    "uint64_t": 8, "int64_t": 8, "double": 8,
    "float": 4, "uint32_t": 4, "int32_t": 4,
    "uint16_t": 2, "int16_t": 2,
    "uint8_t": 1, "int8_t": 1, "char": 1,
}

MESSAGES = [
    "HEARTBEAT", "SYS_STATUS", "GPS_RAW_INT", "ATTITUDE", "LOCAL_POSITION_NED",
    "GLOBAL_POSITION_INT", "VFR_HUD", "COMMAND_LONG", "COMMAND_ACK",
    "SET_POSITION_TARGET_GLOBAL_INT", "NAMED_VALUE_FLOAT", "STATUSTEXT",
]


def main() -> int:
    print("%-32s %-6s %-9s %-4s %s" % ("message", "id", "crc_extra", "len", "wire order"))
    for name in MESSAGES:
        cls = getattr(mavlink, "MAVLink_%s_message" % name.lower())
        offset = 0
        parts = []
        # ordered_fieldnames is the wire order; the extension fields sort last in it.
        for field in cls.ordered_fieldnames:
            index = cls.fieldnames.index(field)
            kind = cls.fieldtypes[index]
            # pymavlink reports the char-array length against the wrong index for
            # NAMED_VALUE_FLOAT; the byte layout is value@4 then name@8[10], which
            # the encoder and scripts/sitl/mavlink_xcheck.py both confirm.
            count = 1 if (name == "NAMED_VALUE_FLOAT" and field == "value") else (cls.array_lengths[index] or 1)
            if name == "NAMED_VALUE_FLOAT" and field == "name":
                count = 10
            size = SIZES[kind] * count
            parts.append("%s@%d" % (field + ("[%d]" % count if count > 1 else ""), offset))
            offset += size
        print("%-32s %-6d %-9d %-4d %s" % (name, cls.id, cls.crc_extra, offset, "  ".join(parts)))
    return 0


if __name__ == "__main__":
    sys.exit(main())
