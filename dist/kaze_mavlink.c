/* kaze_mavlink.c -- MAVLink 2.0 framing, CRC and the SITL bridge's message set.
 * See kaze_mavlink.h for where the wire tables come from. */
#include "kaze_mavlink.h"

#include <string.h>

static const kz_mav_info KZ_MAV_TABLE[] = {
    /* id, crc_extra, payload length */
    {0u, 50u, 9u},     /* HEARTBEAT */
    {1u, 124u, 43u},   /* SYS_STATUS (43 with the sensor-bitmask extensions; older
                        * dialects send 31, and the fields this bridge reads sit
                        * before either tail) */
    {24u, 24u, 52u},   /* GPS_RAW_INT */
    {30u, 39u, 28u},   /* ATTITUDE */
    {32u, 185u, 28u},  /* LOCAL_POSITION_NED */
    {33u, 104u, 28u},  /* GLOBAL_POSITION_INT */
    {74u, 20u, 20u},   /* VFR_HUD */
    {76u, 152u, 33u},  /* COMMAND_LONG */
    {77u, 143u, 10u},  /* COMMAND_ACK */
    {86u, 5u, 53u},    /* SET_POSITION_TARGET_GLOBAL_INT */
    {251u, 170u, 18u}, /* NAMED_VALUE_FLOAT */
    {253u, 83u, 54u},  /* STATUSTEXT */
};

bool kz_mav_info_of(uint32_t msgid, kz_mav_info *out) {
    size_t i;
    for (i = 0; i < sizeof(KZ_MAV_TABLE) / sizeof(KZ_MAV_TABLE[0]); i++) {
        if (KZ_MAV_TABLE[i].id == msgid) {
            if (out != NULL) *out = KZ_MAV_TABLE[i];
            return true;
        }
    }
    return false;
}

uint16_t kz_mav_crc(const uint8_t *buf, size_t len, uint8_t extra) {
    uint16_t crc = 0xFFFFu;
    size_t i;
    int pass;
    for (pass = 0; pass < 2; pass++) {
        size_t n = (pass == 0) ? len : 1u;
        for (i = 0; i < n; i++) {
            uint8_t b = (pass == 0) ? buf[i] : extra;
            uint8_t tmp = (uint8_t)(b ^ (crc & 0xFFu));
            tmp = (uint8_t)(tmp ^ (uint8_t)(tmp << 4));
            crc = (uint16_t)((crc >> 8) ^ ((uint16_t)tmp << 8) ^ ((uint16_t)tmp << 3) ^
                             ((uint16_t)tmp >> 4));
        }
    }
    return crc;
}

size_t kz_mav_build(uint8_t *out, size_t cap, uint8_t seq, uint8_t sysid, uint8_t compid,
                    uint32_t msgid, const uint8_t *payload, size_t payload_len) {
    kz_mav_info info;
    size_t i;
    size_t total;
    uint16_t crc;
    if (out == NULL || payload_len > KZ_MAV_MAX_PAYLOAD) return 0;
    if (!kz_mav_info_of(msgid, &info)) return 0;
    if (payload_len > info.len) return 0;
    total = 12u + payload_len;
    if (total > cap) return 0;

    out[0] = KZ_MAV_MAGIC;
    out[1] = (uint8_t)payload_len;
    out[2] = 0u; /* incompat flags: no signing */
    out[3] = 0u; /* compat flags */
    out[4] = seq;
    out[5] = sysid;
    out[6] = compid;
    out[7] = (uint8_t)(msgid & 0xFFu);
    out[8] = (uint8_t)((msgid >> 8) & 0xFFu);
    out[9] = (uint8_t)((msgid >> 16) & 0xFFu);
    for (i = 0; i < payload_len; i++) out[10 + i] = payload[i];

    crc = kz_mav_crc(out + 1, 9u + payload_len, info.crc_extra);
    out[10 + payload_len] = (uint8_t)(crc & 0xFFu);
    out[11 + payload_len] = (uint8_t)(crc >> 8);
    return total;
}

int kz_mav_parse(const uint8_t *buf, size_t len, kz_mav_frame *out) {
    uint8_t payload_len;
    size_t total;
    uint32_t msgid;
    kz_mav_info info;
    uint16_t want;
    uint16_t got;
    if (buf == NULL || len < 1u) return 0;
    if (buf[0] != KZ_MAV_MAGIC) return -1;
    if (len < 12u) return 0;
    payload_len = buf[1];
    total = 12u + (size_t)payload_len;
    if (len < total) return 0;
    msgid = (uint32_t)buf[7] | ((uint32_t)buf[8] << 8) | ((uint32_t)buf[9] << 16);
    if (!kz_mav_info_of(msgid, &info)) return -1;
    want = kz_mav_crc(buf + 1, 9u + (size_t)payload_len, info.crc_extra);
    got = (uint16_t)((uint16_t)buf[10 + payload_len] |
                     ((uint16_t)buf[11 + payload_len] << 8));
    if (want != got) return -1;
    if (out != NULL) {
        out->seq = buf[4];
        out->sysid = buf[5];
        out->compid = buf[6];
        out->incompat_flags = buf[2];
        out->compat_flags = buf[3];
        out->msgid = msgid;
        out->payload_len = (size_t)payload_len;
        out->payload = buf + 10;
    }
    return (int)total;
}

float kz_mav_f32(const uint8_t *p, size_t off) {
    union { uint32_t u; float f; } v;
    v.u = (uint32_t)p[off] | ((uint32_t)p[off + 1] << 8) | ((uint32_t)p[off + 2] << 16) |
          ((uint32_t)p[off + 3] << 24);
    return v.f;
}

uint32_t kz_mav_u32(const uint8_t *p, size_t off) {
    return (uint32_t)p[off] | ((uint32_t)p[off + 1] << 8) | ((uint32_t)p[off + 2] << 16) |
           ((uint32_t)p[off + 3] << 24);
}

int32_t kz_mav_i32(const uint8_t *p, size_t off) {
    return (int32_t)kz_mav_u32(p, off);
}

uint16_t kz_mav_u16(const uint8_t *p, size_t off) {
    return (uint16_t)((uint16_t)p[off] | ((uint16_t)p[off + 1] << 8));
}

int16_t kz_mav_i16(const uint8_t *p, size_t off) {
    return (int16_t)kz_mav_u16(p, off);
}

uint8_t kz_mav_u8(const uint8_t *p, size_t off) { return p[off]; }

int8_t kz_mav_i8(const uint8_t *p, size_t off) { return (int8_t)p[off]; }

void kz_mav_put_f32(uint8_t *p, size_t off, float v) {
    union { uint32_t u; float f; } x;
    x.f = v;
    kz_mav_put_u32(p, off, x.u);
}

void kz_mav_put_u32(uint8_t *p, size_t off, uint32_t v) {
    p[off] = (uint8_t)(v & 0xFFu);
    p[off + 1] = (uint8_t)((v >> 8) & 0xFFu);
    p[off + 2] = (uint8_t)((v >> 16) & 0xFFu);
    p[off + 3] = (uint8_t)((v >> 24) & 0xFFu);
}

void kz_mav_put_i32(uint8_t *p, size_t off, int32_t v) { kz_mav_put_u32(p, off, (uint32_t)v); }

void kz_mav_put_u16(uint8_t *p, size_t off, uint16_t v) {
    p[off] = (uint8_t)(v & 0xFFu);
    p[off + 1] = (uint8_t)((v >> 8) & 0xFFu);
}

void kz_mav_put_i16(uint8_t *p, size_t off, int16_t v) { kz_mav_put_u16(p, off, (uint16_t)v); }

void kz_mav_put_u8(uint8_t *p, size_t off, uint8_t v) { p[off] = v; }

size_t kz_mav_heartbeat(uint8_t *p, uint8_t type, uint8_t autopilot, uint8_t base_mode,
                        uint32_t custom_mode, uint8_t system_status) {
    memset(p, 0, 9u);
    kz_mav_put_u32(p, 0u, custom_mode);
    kz_mav_put_u8(p, 4u, type);
    kz_mav_put_u8(p, 5u, autopilot);
    kz_mav_put_u8(p, 6u, base_mode);
    kz_mav_put_u8(p, 7u, system_status);
    kz_mav_put_u8(p, 8u, 3u); /* mavlink_version */
    return 9u;
}

size_t kz_mav_command_long(uint8_t *p, uint16_t command, uint8_t target_system,
                           uint8_t target_component, float p1, float p2, float p3,
                           float p4, float p5, float p6, float p7) {
    memset(p, 0, 33u);
    kz_mav_put_f32(p, 0u, p1);
    kz_mav_put_f32(p, 4u, p2);
    kz_mav_put_f32(p, 8u, p3);
    kz_mav_put_f32(p, 12u, p4);
    kz_mav_put_f32(p, 16u, p5);
    kz_mav_put_f32(p, 20u, p6);
    kz_mav_put_f32(p, 24u, p7);
    kz_mav_put_u16(p, 28u, command);
    kz_mav_put_u8(p, 30u, target_system);
    kz_mav_put_u8(p, 31u, target_component);
    kz_mav_put_u8(p, 32u, 0u);
    return 33u;
}

size_t kz_mav_set_position_global_int(uint8_t *p, uint32_t time_boot_ms, int32_t lat_int,
                                      int32_t lon_int, float alt, uint16_t type_mask,
                                      uint8_t target_system, uint8_t target_component,
                                      uint8_t frame) {
    memset(p, 0, 53u);
    kz_mav_put_u32(p, 0u, time_boot_ms);
    kz_mav_put_i32(p, 4u, lat_int);
    kz_mav_put_i32(p, 8u, lon_int);
    kz_mav_put_f32(p, 12u, alt);
    kz_mav_put_u16(p, 48u, type_mask);
    kz_mav_put_u8(p, 50u, target_system);
    kz_mav_put_u8(p, 51u, target_component);
    kz_mav_put_u8(p, 52u, frame);
    return 53u;
}
