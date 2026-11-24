/* mavlink_probe.c -- the C side of the codec cross-check.
 *
 * scripts/sitl/mavlink_xcheck.py builds frames with pymavlink, hands them to
 * this program, and compares what the C codec decodes against what it encoded;
 * it also asks this program to build frames and parses them with pymavlink.
 * Two independent implementations must agree, byte for byte, or the bridge is
 * only talking to itself.
 *
 *   mavlink_probe parse <hex>     one frame on argv, fields on stdout
 *   mavlink_probe build           the frames this codec builds, hex on stdout
 */
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "kaze_mavlink.h"

static size_t unhex(const char* s, uint8_t* out, size_t cap) {
    size_t n = 0;
    int hi = -1;
    for (; *s; s++) {
        int v;
        char c = *s;
        if (c == ' ' || c == '\n' || c == '\t') continue;
        if (c >= '0' && c <= '9') v = c - '0';
        else if (c >= 'a' && c <= 'f') v = c - 'a' + 10;
        else if (c >= 'A' && c <= 'F') v = c - 'A' + 10;
        else return 0;
        if (hi < 0) hi = v;
        else {
            if (n >= cap) return 0;
            out[n++] = (uint8_t)((hi << 4) | v);
            hi = -1;
        }
    }
    return n;
}

static void put_hex(const uint8_t* b, size_t n) {
    size_t i;
    for (i = 0; i < n; i++) printf("%02x", b[i]);
    printf("\n");
}

int main(int argc, char** argv) {
    if (argc >= 3 && strcmp(argv[1], "parse") == 0) {
        uint8_t buf[512];
        size_t n = unhex(argv[2], buf, sizeof buf);
        kz_mav_frame f;
        int used;
        if (n == 0) { printf("unhex-failed\n"); return 1; }
        used = kz_mav_parse(buf, n, &f);
        if (used <= 0) { printf("rejected used=%d\n", used); return 1; }
        printf("id=%u len=%zu used=%d seq=%u sys=%u comp=%u\n", (unsigned)f.msgid,
               f.payload_len, used, (unsigned)f.seq, (unsigned)f.sysid, (unsigned)f.compid);
        switch (f.msgid) {
            case KZ_MAV_MSG_HEARTBEAT:
                printf("heartbeat type=%u autopilot=%u base_mode=%u custom_mode=%u status=%u\n",
                       (unsigned)kz_mav_u8(f.payload, 4), (unsigned)kz_mav_u8(f.payload, 5),
                       (unsigned)kz_mav_u8(f.payload, 6), (unsigned)kz_mav_u32(f.payload, 0),
                       (unsigned)kz_mav_u8(f.payload, 7));
                break;
            case KZ_MAV_MSG_GLOBAL_POSITION_INT:
                printf("global lat=%d lon=%d alt=%d rel=%d vx=%d vy=%d vz=%d hdg=%u\n",
                       kz_mav_i32(f.payload, 4), kz_mav_i32(f.payload, 8),
                       kz_mav_i32(f.payload, 12), kz_mav_i32(f.payload, 16),
                       (int)kz_mav_i16(f.payload, 20), (int)kz_mav_i16(f.payload, 22),
                       (int)kz_mav_i16(f.payload, 24), (unsigned)kz_mav_u16(f.payload, 26));
                break;
            case KZ_MAV_MSG_COMMAND_ACK:
                /* MAVLink 2 keeps extension fields after the base layout, so
                 * command/result/progress sit at 0/2/3 and result_param2 (an
                 * extension) at 4. Reading them by size order gets this wrong. */
                printf("ack command=%u result=%u progress=%u\n",
                       (unsigned)kz_mav_u16(f.payload, 0), (unsigned)kz_mav_u8(f.payload, 2),
                       (unsigned)kz_mav_u8(f.payload, 3));
                break;
            case KZ_MAV_MSG_GPS_RAW_INT:
                printf("gps fix=%u sats=%u eph=%u\n", (unsigned)kz_mav_u8(f.payload, 28),
                       (unsigned)kz_mav_u8(f.payload, 29), (unsigned)kz_mav_u16(f.payload, 20));
                break;
            case KZ_MAV_MSG_ATTITUDE:
                printf("attitude roll=%.4f pitch=%.4f yaw=%.4f\n", (double)kz_mav_f32(f.payload, 4),
                       (double)kz_mav_f32(f.payload, 8), (double)kz_mav_f32(f.payload, 12));
                break;
            case KZ_MAV_MSG_VFR_HUD:
                printf("hud groundspeed=%.3f alt=%.3f\n", (double)kz_mav_f32(f.payload, 4),
                       (double)kz_mav_f32(f.payload, 8));
                break;
            case KZ_MAV_MSG_NAMED_VALUE_FLOAT: {
                char name[11];
                int i;
                for (i = 0; i < 10; i++) name[i] = (char)kz_mav_u8(f.payload, (size_t)(8 + i));
                name[10] = '\0';
                printf("named name=%s value=%.4f\n", name, (double)kz_mav_f32(f.payload, 4));
                break;
            }
            default:
                printf("other\n");
                break;
        }
        return 0;
    }

    if (argc >= 2 && strcmp(argv[1], "build") == 0) {
        uint8_t payload[64];
        uint8_t frame[KZ_MAV_MAX_FRAME];
        size_t len, n;
        /* Same seq/sysid/compid the Python side uses, so bytes are comparable. */
        len = kz_mav_heartbeat(payload, 2u, 3u, 128u, 4u, 4u);
        n = kz_mav_build(frame, sizeof frame, 7, 1, 1, KZ_MAV_MSG_HEARTBEAT, payload, len);
        put_hex(frame, n);
        len = kz_mav_command_long(payload, KZ_MAV_CMD_COMPONENT_ARM_DISARM, 1, 1, 1.0f, 0, 0, 0, 0, 0, 0);
        n = kz_mav_build(frame, sizeof frame, 8, 255, 190, KZ_MAV_MSG_COMMAND_LONG, payload, len);
        put_hex(frame, n);
        len = kz_mav_set_position_global_int(payload, 0x01020304u, 476000000, 85000000, 15.0f,
                                             KZ_MAV_TYPEMASK_POS_ONLY, 1, 1,
                                             KZ_MAV_FRAME_GLOBAL_RELATIVE_ALT_INT);
        n = kz_mav_build(frame, sizeof frame, 9, 255, 190, KZ_MAV_MSG_SET_POSITION_TARGET_GLOBAL_INT,
                         payload, len);
        put_hex(frame, n);
        return 0;
    }

    fprintf(stderr, "usage: mavlink_probe parse <hex> | build\n");
    return 2;
}
