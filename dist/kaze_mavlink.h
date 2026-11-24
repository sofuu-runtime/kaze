/* kaze_mavlink.h -- MAVLink 2.0 framing and the message subset the SITL
 * bridge speaks. No code here knows about Kaze; a second implementation of the
 * same protocol lives in scripts/sitl/kaze_sitl.py (pymavlink), and
 * scripts/phase15-gates.sh cross-checks the two: a frame this file builds must
 * decode in pymavlink, and a frame pymavlink builds must decode here.
 *
 * Wire tables (message id, CRC_EXTRA, payload length, field offsets) were read
 * from the dialect itself, not remembered:
 *
 *   python3 -c 'from pymavlink.dialects.v20 import ardupilotmega as m; ...'
 *
 * MAVLink 2 frame:
 *   0xFD | len u8 | incompat u8 | compat u8 | seq u8 | sysid u8 | compid u8
 *   | msgid u24le | payload[len] | crc u16le
 * where crc is CRC-16/MCRF4XX over len..payload followed by one CRC_EXTRA byte.
 */
#ifndef KAZE_MAVLINK_H
#define KAZE_MAVLINK_H

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#define KZ_MAV_MAGIC 0xFD
#define KZ_MAV_MAX_PAYLOAD 255
#define KZ_MAV_MAX_FRAME (12 + KZ_MAV_MAX_PAYLOAD)

/* Message ids this bridge uses. */
#define KZ_MAV_MSG_HEARTBEAT 0
#define KZ_MAV_MSG_SYS_STATUS 1
#define KZ_MAV_MSG_GPS_RAW_INT 24
#define KZ_MAV_MSG_ATTITUDE 30
#define KZ_MAV_MSG_LOCAL_POSITION_NED 32
#define KZ_MAV_MSG_GLOBAL_POSITION_INT 33
#define KZ_MAV_MSG_VFR_HUD 74
#define KZ_MAV_MSG_COMMAND_LONG 76
#define KZ_MAV_MSG_COMMAND_ACK 77
#define KZ_MAV_MSG_SET_POSITION_TARGET_GLOBAL_INT 86
#define KZ_MAV_MSG_NAMED_VALUE_FLOAT 251
#define KZ_MAV_MSG_STATUSTEXT 253

/* Commands (MAV_CMD) and flags (MAV_MODE_FLAG) the bridge sends. */
#define KZ_MAV_CMD_NAV_RETURN_TO_LAUNCH 20
#define KZ_MAV_CMD_NAV_LAND 21
#define KZ_MAV_CMD_NAV_TAKEOFF 22
#define KZ_MAV_CMD_DO_PAUSE_CONTINUE 193
#define KZ_MAV_CMD_COMPONENT_ARM_DISARM 400

#define KZ_MAV_MODE_FLAG_SAFETY_ARMED 128
#define KZ_MAV_MODE_FLAG_GUIDED_ENABLED 8
#define KZ_MAV_MODE_FLAG_CUSTOM_MODE_ENABLED 1
#define KZ_MAV_RESULT_ACCEPTED 0
#define KZ_MAV_FRAME_GLOBAL_RELATIVE_ALT_INT 6
#define KZ_MAV_TYPE_GCS 6
#define KZ_MAV_AUTOPILOT_INVALID 8
#define KZ_MAV_STATE_ACTIVE 4
/* Position only: ignore velocity, acceleration, force, yaw and yaw rate. */
#define KZ_MAV_TYPEMASK_POS_ONLY 0x0DF8u

typedef struct {
    uint8_t seq;
    uint8_t sysid;
    uint8_t compid;
    uint8_t incompat_flags;
    uint8_t compat_flags;
    uint32_t msgid;
    size_t payload_len;
    const uint8_t *payload;
} kz_mav_frame;

typedef struct {
    uint32_t id;
    uint8_t crc_extra;
    uint8_t len;
} kz_mav_info;

/* The table entry for a message id, or false when this bridge does not speak it. */
bool kz_mav_info_of(uint32_t msgid, kz_mav_info *out);

/* CRC-16/MCRF4XX, the checksum MAVLink uses, with one extra byte appended. */
uint16_t kz_mav_crc(const uint8_t *buf, size_t len, uint8_t extra);

/* Build a frame into out. Returns the frame length, or 0 when it cannot fit. */
size_t kz_mav_build(uint8_t *out, size_t cap, uint8_t seq, uint8_t sysid, uint8_t compid,
                    uint32_t msgid, const uint8_t *payload, size_t payload_len);

/* Parse one frame at the start of buf. Returns the bytes consumed (0 when there
 * is no complete frame yet, -1 when the checksum or the framing is wrong, in
 * which case the caller should resynchronise on the next 0xFD). */
int kz_mav_parse(const uint8_t *buf, size_t len, kz_mav_frame *out);

/* Payload field access, little endian, by wire offset. */
float kz_mav_f32(const uint8_t *p, size_t off);
uint32_t kz_mav_u32(const uint8_t *p, size_t off);
int32_t kz_mav_i32(const uint8_t *p, size_t off);
uint16_t kz_mav_u16(const uint8_t *p, size_t off);
int16_t kz_mav_i16(const uint8_t *p, size_t off);
uint8_t kz_mav_u8(const uint8_t *p, size_t off);
int8_t kz_mav_i8(const uint8_t *p, size_t off);
void kz_mav_put_f32(uint8_t *p, size_t off, float v);
void kz_mav_put_u32(uint8_t *p, size_t off, uint32_t v);
void kz_mav_put_i32(uint8_t *p, size_t off, int32_t v);
void kz_mav_put_u16(uint8_t *p, size_t off, uint16_t v);
void kz_mav_put_i16(uint8_t *p, size_t off, int16_t v);
void kz_mav_put_u8(uint8_t *p, size_t off, uint8_t v);

/* Named payload builders for the four messages the bridge sends, and the
 * layout constants for the ones it reads. Each returns the payload length. */
size_t kz_mav_heartbeat(uint8_t *p, uint8_t type, uint8_t autopilot, uint8_t base_mode,
                        uint32_t custom_mode, uint8_t system_status);
size_t kz_mav_command_long(uint8_t *p, uint16_t command, uint8_t target_system,
                           uint8_t target_component, float p1, float p2, float p3,
                           float p4, float p5, float p6, float p7);
size_t kz_mav_set_position_global_int(uint8_t *p, uint32_t time_boot_ms, int32_t lat_int,
                                      int32_t lon_int, float alt, uint16_t type_mask,
                                      uint8_t target_system, uint8_t target_component,
                                      uint8_t frame);

#endif /* KAZE_MAVLINK_H */
