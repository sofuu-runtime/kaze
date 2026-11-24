/* kaze_sitl_client.c -- the drone capability's SITL link.
 *
 * Included into an emitted program's translation unit when the program is built
 * with -DKAZE_SITL_LINK (scripts/sitl builds that way). Without it the same
 * call sites still fail closed with E_CAP_MISSING, so a normal build can never
 * silently simulate anything.
 *
 * Selection is explicit and labelled: KAZE_SITL names the vehicle
 * ("udp:127.0.0.1:14550" or "127.0.0.1:14550"). The first drone call prints one
 * line to stderr saying which simulated vehicle it linked to, because a
 * simulation result is not a hardware result.
 *
 * Every command is synchronous in this mode: arm waits for the armed heartbeat,
 * takeoff waits for altitude, go to waits for arrival, land and RTL wait for
 * disarm. KAZE_SITL_TIMEOUT_MS bounds each wait (default 60000) and
 * KAZE_SITL_LINK_TIMEOUT_MS bounds silence from the vehicle (default 3000); a
 * timeout is E_KAZE_LINK_TIMEOUT and exit status 70, never a fabricated ack.
 */
#include <errno.h>
#include <fcntl.h>
#include <arpa/inet.h>
#include <netdb.h>
#include <netinet/in.h>
#include <sys/socket.h>
#include <sys/time.h>
#include <time.h>
#include <unistd.h>

#include "kaze_mavlink.h"

static int kz_sitl_sock = -1;
static int kz_sitl_up = 0;
static uint8_t kz_sitl_seq = 0;
static uint8_t kz_sitl_sysid = 255;   /* GCS */
static uint8_t kz_sitl_compid = 190;  /* MAV_COMP_ID_MISSIONPLANNER */
static struct sockaddr_in kz_sitl_addr;
static double kz_sitl_last_tx = 0.0;
static double kz_sitl_last_rx = 0.0;
static uint32_t kz_sitl_boot_ms = 0;
static uint8_t kz_sitl_buf[4096];
static uint8_t kz_sitl_frame[KZ_MAV_MAX_FRAME];

/* Telemetry cache, refreshed by every pump. */
static struct {
    int have_heartbeat;
    int armed;
    int mode;
    int failsafe;
    double lat, lon, alt_rel, alt_msl, vn, ve, vd, yaw, battery, hdop, sats, fix, groundspeed;
    double wind_speed, wind_dir, roll, pitch;
    int on_target;
    uint32_t vehicle_ms;   /* the vehicle's own clock, from its telemetry */
    uint32_t on_target_ms; /* vehicle clock when the arrival flag was last set */
    int leg_seq;           /* the vehicle's leg counter, when it publishes one */
    int statustexts;
    char last_status[51];
} kz_tel;

KZ_MAYBE_UNUSED static double kz_now_s(void) {
    struct timeval tv;
    gettimeofday(&tv, NULL);
    return (double)tv.tv_sec + (double)tv.tv_usec / 1e6;
}

KZ_MAYBE_UNUSED static long kz_env_i(const char* name, long dflt) {
    const char* v = getenv(name);
    char* end = NULL;
    long n;
    if (v == NULL || v[0] == '\0') return dflt;
    n = strtol(v, &end, 10);
    if (end == v || n <= 0) return dflt;
    return n;
}

KZ_MAYBE_UNUSED static void kz_sitl_fail(const char* code, const char* detail) {
    fprintf(stderr, "%s: %s\n", code, detail);
    exit(70);
}

KZ_MAYBE_UNUSED static void kz_sitl_send(uint32_t msgid, const uint8_t* payload, size_t len) {
    size_t n;
    ssize_t sent;
    if (kz_sitl_sock < 0) return;
    n = kz_mav_build(kz_sitl_frame, sizeof kz_sitl_frame, kz_sitl_seq++, kz_sitl_sysid,
                     kz_sitl_compid, msgid, payload, len);
    if (n == 0) kz_sitl_fail("E_KAZE_LINK", "a command could not be framed for the vehicle");
    sent = sendto(kz_sitl_sock, kz_sitl_frame, n, 0, (struct sockaddr*)&kz_sitl_addr,
                  sizeof kz_sitl_addr);
    if (sent < 0) kz_sitl_fail("E_KAZE_LINK", "the vehicle link refused a send");
    kz_sitl_last_tx = kz_now_s();
}

/* MAVLink 2 truncates trailing zero bytes, so a short payload is a valid one:
 * copy it into a zeroed full-size buffer before reading a field past its end. */
KZ_MAYBE_UNUSED static void kz_sitl_on_message(const kz_mav_frame* f) {
    uint8_t padded[56];
    kz_mav_info info;
    const uint8_t* p = padded;
    size_t i;
    memset(padded, 0, sizeof padded);
    if (kz_mav_info_of(f->msgid, &info)) {
        size_t n = f->payload_len;
        if (n > info.len) n = info.len;
        if (n > sizeof padded) n = sizeof padded;
        for (i = 0; i < n; i++) padded[i] = f->payload[i];
    } else {
        return;
    }
    kz_sitl_last_rx = kz_now_s();
    switch (f->msgid) {
        case KZ_MAV_MSG_HEARTBEAT:
            kz_tel.have_heartbeat = 1;
            kz_tel.armed = (kz_mav_u8(p, 6) & KZ_MAV_MODE_FLAG_SAFETY_ARMED) ? 1 : 0;
            kz_tel.mode = (int)kz_mav_u32(p, 0);
            break;
        case KZ_MAV_MSG_GLOBAL_POSITION_INT:
            kz_sitl_boot_ms = kz_mav_u32(p, 0);
            kz_tel.vehicle_ms = kz_sitl_boot_ms;
            kz_tel.lat = (double)kz_mav_i32(p, 4) / 1e7;
            kz_tel.lon = (double)kz_mav_i32(p, 8) / 1e7;
            kz_tel.alt_msl = (double)kz_mav_i32(p, 12) / 1000.0;
            kz_tel.alt_rel = (double)kz_mav_i32(p, 16) / 1000.0;
            kz_tel.vn = (double)kz_mav_i16(p, 20) / 100.0;
            kz_tel.ve = (double)kz_mav_i16(p, 22) / 100.0;
            kz_tel.vd = (double)kz_mav_i16(p, 24) / 100.0;
            break;
        case KZ_MAV_MSG_ATTITUDE:
            kz_tel.roll = (double)kz_mav_f32(p, 4);
            kz_tel.pitch = (double)kz_mav_f32(p, 8);
            kz_tel.yaw = (double)kz_mav_f32(p, 12);
            break;
        case KZ_MAV_MSG_SYS_STATUS:
            /* battery_remaining follows the sensor bitmasks and the four
             * counters; the probe in scripts/sitl/mavlink_layout.py pins it. */
            kz_tel.battery = (double)kz_mav_i8(p, 30);
            break;
        case KZ_MAV_MSG_GPS_RAW_INT:
            /* The base layout ends at 29: alt_ellipsoid and the four accuracy
             * fields are extensions appended after it, so fix_type and
             * satellites_visible are at 28 and 29, not at the very end. */
            kz_tel.fix = (double)kz_mav_u8(p, 28);
            kz_tel.sats = (double)kz_mav_u8(p, 29);
            kz_tel.hdop = (double)kz_mav_u16(p, 20) / 100.0;
            break;
        case KZ_MAV_MSG_VFR_HUD:
            kz_tel.groundspeed = (double)kz_mav_f32(p, 4);
            break;
        case KZ_MAV_MSG_NAMED_VALUE_FLOAT: {
            /* a vehicle may publish sim/servo quantities; the twin publishes
             * wind, failsafe and its own arrival flag. */
            /* The digital twin publishes wind and failsafe state this way, which
             * is the standard MAVLink channel for sim-only quantities. */
            char name[11];
            int i;
            for (i = 0; i < 10; i++) {
                name[i] = (char)kz_mav_u8(p, (size_t)(8 + i));
                if (name[i] == '\0') break;
            }
            name[10] = '\0';
            if (strcmp(name, "wind_speed") == 0) kz_tel.wind_speed = (double)kz_mav_f32(p, 4);
            if (strcmp(name, "wind_dir") == 0) kz_tel.wind_dir = (double)kz_mav_f32(p, 4);
            if (strcmp(name, "failsafe") == 0) kz_tel.failsafe = (kz_mav_f32(p, 4) > 0.5f) ? 1 : 0;
            if (strcmp(name, "on_target") == 0) {
                kz_tel.on_target = (kz_mav_f32(p, 4) > 0.5f) ? 1 : 0;
                kz_tel.on_target_ms = kz_mav_u32(p, 0);
            }
            if (strcmp(name, "leg_seq") == 0) kz_tel.leg_seq = (int)kz_mav_f32(p, 4);
            break;
        }
        case KZ_MAV_MSG_STATUSTEXT: {
            int i;
            kz_tel.statustexts += 1;
            for (i = 0; i < 50; i++) {
                char c = (char)kz_mav_u8(p, (size_t)(1 + i));
                kz_tel.last_status[i] = c;
                if (c == '\0') break;
            }
            kz_tel.last_status[50] = '\0';
            break;
        }
        case KZ_MAV_MSG_COMMAND_ACK:
            break;
        default:
            break;
    }
}

KZ_MAYBE_UNUSED static void kz_sitl_pump(void) {
    int guard = 0;
    for (;;) {
        ssize_t n = recvfrom(kz_sitl_sock, kz_sitl_buf, sizeof kz_sitl_buf, 0, NULL, NULL);
        size_t off = 0;
        if (n <= 0) break;
        if (guard++ > 256) break;
        while (off < (size_t)n) {
            kz_mav_frame f;
            int used = kz_mav_parse(kz_sitl_buf + off, (size_t)n - off, &f);
            if (used == 0) break;
            if (used < 0) {
                off += 1;
                continue;
            }
            kz_sitl_on_message(&f);
            off += (size_t)used;
        }
    }
    /* A heartbeat at 200 Hz keeps a simulated radio link alive in real time
     * without depending on how fast the vehicle's clock runs. */
    if (kz_now_s() - kz_sitl_last_tx >= 0.005) {
        uint8_t hb[9];
        size_t len = kz_mav_heartbeat(hb, KZ_MAV_TYPE_GCS, KZ_MAV_AUTOPILOT_INVALID, 0u, 0u,
                                      KZ_MAV_STATE_ACTIVE);
        kz_sitl_send(KZ_MAV_MSG_HEARTBEAT, hb, len);
    }
}

KZ_MAYBE_UNUSED static void kz_sitl_connect(void) {
    const char* spec;
    char host[256];
    const char* port_s;
    int port;
    struct addrinfo hints;
    struct addrinfo* res = NULL;
    size_t i;
    int flags;
    if (kz_sitl_up) return;
    spec = getenv("KAZE_SITL");
    if (spec == NULL || spec[0] == '\0') {
        kz_sitl_fail("E_KAZE_LINK", "KAZE_SITL is not set; a drone call needs an explicitly selected vehicle");
    }
    if (strncmp(spec, "udp:", 4) == 0) spec += 4;
    port_s = strrchr(spec, ':');
    if (port_s == NULL) {
        kz_sitl_fail("E_KAZE_LINK", "KAZE_SITL must name host:port, for example udp:127.0.0.1:14550");
    }
    port = atoi(port_s + 1);
    for (i = 0; i < sizeof host - 1 && spec + i < port_s; i++) host[i] = spec[i];
    host[i] = '\0';
    if (port <= 0 || port > 65535) {
        kz_sitl_fail("E_KAZE_LINK", "KAZE_SITL has a port outside 1..65535");
    }

    memset(&hints, 0, sizeof hints);
    hints.ai_family = AF_INET;
    hints.ai_socktype = SOCK_DGRAM;
    if (getaddrinfo(host, NULL, &hints, &res) != 0 || res == NULL) {
        kz_sitl_fail("E_KAZE_LINK", "KAZE_SITL names a host that does not resolve");
    }
    memset(&kz_sitl_addr, 0, sizeof kz_sitl_addr);
    kz_sitl_addr.sin_family = AF_INET;
    kz_sitl_addr.sin_port = htons((unsigned short)port);
    kz_sitl_addr.sin_addr = ((struct sockaddr_in*)res->ai_addr)->sin_addr;
    freeaddrinfo(res);

    kz_sitl_sock = socket(AF_INET, SOCK_DGRAM, 0);
    if (kz_sitl_sock < 0) kz_sitl_fail("E_KAZE_LINK", "cannot open a UDP socket for the vehicle link");
    flags = fcntl(kz_sitl_sock, F_GETFL, 0);
    if (flags >= 0) (void)fcntl(kz_sitl_sock, F_SETFL, flags | O_NONBLOCK);

    kz_sitl_up = 1;
    kz_tel.fix = 0.0;
    kz_tel.sats = 0.0;
    fprintf(stderr, "kaze-sitl: linked to udp:%s:%d (simulated vehicle; not hardware results)\n",
            host, port);
    kz_sitl_last_rx = kz_now_s();
    kz_sitl_pump();
}

KZ_MAYBE_UNUSED static int kz_sitl_enabled(void) {
    const char* spec = getenv("KAZE_SITL");
    return (spec != NULL && spec[0] != '\0') ? 1 : 0;
}

typedef int (*kz_sitl_cond)(void);

KZ_MAYBE_UNUSED static void kz_sitl_wait(kz_sitl_cond cond, const char* what,
                                          int disarm_is_abort) {
    double start = kz_now_s();
    double limit = (double)kz_env_i("KAZE_SITL_TIMEOUT_MS", 60000) / 1000.0;
    double link_limit = (double)kz_env_i("KAZE_SITL_LINK_TIMEOUT_MS", 3000) / 1000.0;
    for (;;) {
        kz_sitl_pump();
        if (disarm_is_abort == 1 && kz_tel.have_heartbeat == 1 && kz_tel.armed == 0) {
            /* The autopilot disarmed while it was supposed to be flying the leg:
             * a failsafe abort, not a leg in progress. */
            kz_sitl_fail("E_SAFETY_VIOLATION",
                         "the vehicle disarmed mid-mission (failsafe abort); it is not flying this leg");
        }
        if (cond != NULL && cond() == 1) return;
        if (kz_now_s() - start > limit) {
            char msg[256];
            snprintf(msg, sizeof msg, "the vehicle did not finish %s inside %.1f s", what, limit);
            kz_sitl_fail("E_KAZE_LINK_TIMEOUT", msg);
        }
        if (kz_now_s() - kz_sitl_last_rx > link_limit) {
            char msg[256];
            snprintf(msg, sizeof msg, "no telemetry from the vehicle for %.1f s while waiting for %s",
                     link_limit, what);
            kz_sitl_fail("E_KAZE_LINK_TIMEOUT", msg);
        }
        usleep(1000);
    }
}

KZ_MAYBE_UNUSED static int kz_sitl_wait_armed(void) { return kz_tel.armed == 1 ? 1 : 0; }
KZ_MAYBE_UNUSED static int kz_sitl_wait_disarmed(void) { return (kz_tel.have_heartbeat && kz_tel.armed == 0) ? 1 : 0; }

static double kz_sitl_target_alt;
KZ_MAYBE_UNUSED static int kz_sitl_wait_alt(void) {
    double d = kz_tel.alt_rel - kz_sitl_target_alt;
    if (d < 0) d = -d;
    return (d <= 0.5 && (kz_tel.vd < 0.3 && kz_tel.vd > -0.3)) ? 1 : 0;
}

static double kz_sitl_target_lat, kz_sitl_target_lon;
static uint32_t kz_sitl_arrival_after_ms = 0;
static int kz_sitl_arrival_phase = 0;
static int kz_sitl_arrival_seq = 0;
KZ_MAYBE_UNUSED static int kz_sitl_wait_arrival(void) {
    double dlat = (kz_tel.lat - kz_sitl_target_lat) * 111320.0;
    double dlon = (kz_tel.lon - kz_sitl_target_lon) * 111320.0 *
                  cos(kz_tel.lat * 3.14159265358979323846 / 180.0);
    double d = sqrt(dlat * dlat + dlon * dlon);
    double dz = kz_tel.alt_rel - kz_sitl_target_alt;
    /* A GCS sees a measured position, not the truth: a 0.3 m hole around the
     * target is inside the noise, so arrival needs the vehicle to stay inside a
     * wider window for a moment rather than to touch a point once. */
    static double inside_since = 0.0;
    /* If the vehicle publishes its own arrival flag (the digital twin does),
     * trust it -- but only a sample newer than the command, judged on the
     * vehicle's clock. A flag left over from the previous leg is not this leg's
     * arrival, however recently it reached this socket. */
    /* A vehicle that numbers its legs (the digital twin does) makes arrival
     * unambiguous: this leg's flag is the one that arrives after the counter
     * moved past the value seen when the command was sent. */
    if (kz_tel.leg_seq != 0 && kz_tel.leg_seq > kz_sitl_arrival_seq) {
        return kz_tel.on_target == 1 ? 1 : 0;
    }
    /* Otherwise fall back to measurement with a dwell: a 0.3 m hole around the
     * point is inside the noise, so the vehicle must stay inside a wider window
     * for a moment rather than touch a point once. */
    if (kz_tel.on_target_ms > kz_sitl_arrival_after_ms) {
        if (kz_tel.on_target == 0) kz_sitl_arrival_phase = 1;
        if (kz_sitl_arrival_phase == 1 && kz_tel.on_target == 1) return 1;
    }
    if (dz < 0) dz = -dz;
    if (d <= 1.0 && dz <= 1.5) {
        if (inside_since == 0.0) inside_since = kz_now_s();
        return (kz_now_s() - inside_since >= 0.3) ? 1 : 0;
    }
    inside_since = 0.0;
    return 0;
}

KZ_MAYBE_UNUSED static double kz_arg_num(KzValue v, double dflt) {
    if (v.kind == KZ_VAL_NUM) return (double)v.as.number;
    return dflt;
}

KZ_MAYBE_UNUSED static void kz_sitl_telemetry_answer(const char* field, KzValue* out) {
    if (strcmp(field, "battery") == 0) { *out = kz_num(kz_tel.battery); return; }
    if (strcmp(field, "altitude") == 0) { *out = kz_num(kz_tel.alt_rel); return; }
    if (strcmp(field, "heading") == 0 || strcmp(field, "yaw") == 0) {
        double degd = kz_tel.yaw * 180.0 / 3.14159265358979323846;
        if (degd < 0) degd += 360.0;
        *out = kz_num(degd);
        return;
    }
    if (strcmp(field, "speed") == 0) { *out = kz_num(kz_tel.groundspeed); return; }
    if (strcmp(field, "latitude") == 0) { *out = kz_num(kz_tel.lat); return; }
    if (strcmp(field, "longitude") == 0) { *out = kz_num(kz_tel.lon); return; }
    if (strcmp(field, "satellites") == 0) { *out = kz_num(kz_tel.sats); return; }
    if (strcmp(field, "hdop") == 0) { *out = kz_num(kz_tel.hdop); return; }
    if (strcmp(field, "fix") == 0) { *out = kz_num(kz_tel.fix); return; }
    if (strcmp(field, "armed") == 0) { *out = kz_bool(kz_tel.armed == 1); return; }
    if (strcmp(field, "mode") == 0) { *out = kz_num((double)kz_tel.mode); return; }
    if (strcmp(field, "failsafe") == 0) { *out = kz_bool(kz_tel.failsafe == 1); return; }
    if (strcmp(field, "roll") == 0) { *out = kz_num(kz_tel.roll); return; }
    if (strcmp(field, "pitch") == 0) { *out = kz_num(kz_tel.pitch); return; }
    if (strcmp(field, "status_count") == 0) { *out = kz_num((double)kz_tel.statustexts); return; }
    if (strcmp(field, "last_status") == 0) { *out = kz_str(kz_tel.last_status); return; }
    if (strcmp(field, "wind") == 0) { *out = kz_num(kz_tel.wind_speed); return; }
    if (strcmp(field, "wind_direction") == 0) { *out = kz_num(kz_tel.wind_dir); return; }
    if (strcmp(field, "position") == 0) {
        *out = kz_point(kz_num(kz_tel.lat), kz_num(kz_tel.lon), kz_num(kz_tel.alt_rel));
        return;
    }
    *out = kz_nil();
}

KZ_MAYBE_UNUSED static KzValue kz_sitl_field(const char* family, const char* field) {
    KzValue out = kz_nil();
    if (kz_sitl_enabled() == 0) return out;
    kz_sitl_connect();
    kz_sitl_pump();
    if (strcmp(family, "drone") != 0) return out;
    kz_sitl_telemetry_answer(field, &out);
    return out;
}

KZ_MAYBE_UNUSED static KzValue kz_sitl_call(const char* recv, const char* op, int n, KzValue* args) {
    uint8_t payload[64];
    size_t len;
    if (kz_sitl_enabled() == 0) return kz_nil();
    kz_sitl_connect();
    kz_sitl_pump();
    if (strcmp(recv, "drone") != 0) {
        /* Other families keep failing closed: this link only flies a vehicle. */
        fprintf(stderr, "E_CAP_MISSING: the SITL link has no host for %s.%s\n", recv, op);
        exit(70);
    }

    if (getenv("KAZE_SITL_DEBUG") != NULL) {
        fprintf(stderr, "kaze-sitl: op %s.%s n=%d armed=%d alt=%.2f t=%.2f\n", recv, op, n,
                kz_tel.armed, kz_tel.alt_rel, kz_now_s());
    }
    if (strcmp(op, "arm") == 0) {
        len = kz_mav_command_long(payload, KZ_MAV_CMD_COMPONENT_ARM_DISARM, 1, 1, 1.0f, 0, 0, 0, 0, 0, 0);
        kz_sitl_send(KZ_MAV_MSG_COMMAND_LONG, payload, len);
        kz_sitl_wait(kz_sitl_wait_armed, "arming", 0);
        return kz_bool(true);
    }
    if (strcmp(op, "takeoff") == 0) {
        kz_sitl_target_alt = (n > 0) ? kz_arg_num(args[0], 10.0) : 10.0;
        len = kz_mav_command_long(payload, KZ_MAV_CMD_NAV_TAKEOFF, 1, 1, 0, 0, 0, 0, 0, 0,
                                  (float)kz_sitl_target_alt);
        kz_sitl_send(KZ_MAV_MSG_COMMAND_LONG, payload, len);
        kz_sitl_wait(kz_sitl_wait_alt, "the climb", 1);
        return kz_bool(true);
    }
    if (strcmp(op, "goto") == 0) {
        double lat = 0.0, lon = 0.0, alt = 15.0;
        if (n > 0 && args[0].kind == KZ_VAL_POINT) {
            lat = (double)args[0].as.point.x;
            lon = (double)args[0].as.point.y;
            alt = (double)args[0].as.point.z;
            if (alt <= 0.0) alt = 15.0;
        }
        if (n > 1) alt = kz_arg_num(args[1], alt);
        kz_sitl_target_lat = lat;
        kz_sitl_target_lon = lon;
        kz_sitl_target_alt = alt;
        kz_sitl_arrival_after_ms = kz_tel.vehicle_ms;
        kz_sitl_arrival_phase = 0;
        kz_sitl_arrival_seq = kz_tel.leg_seq;
        len = kz_mav_set_position_global_int(payload, kz_sitl_boot_ms, (int32_t)(lat * 1e7),
                                             (int32_t)(lon * 1e7), (float)alt,
                                             KZ_MAV_TYPEMASK_POS_ONLY, 1, 1,
                                             KZ_MAV_FRAME_GLOBAL_RELATIVE_ALT_INT);
        kz_sitl_send(KZ_MAV_MSG_SET_POSITION_TARGET_GLOBAL_INT, payload, len);
        kz_sitl_wait(kz_sitl_wait_arrival, "the leg", 1);
        return kz_bool(true);
    }
    if (strcmp(op, "land") == 0) {
        len = kz_mav_command_long(payload, KZ_MAV_CMD_NAV_LAND, 1, 1, 0, 0, 0, 0, 0, 0, 0);
        kz_sitl_send(KZ_MAV_MSG_COMMAND_LONG, payload, len);
        kz_sitl_wait(kz_sitl_wait_disarmed, "the landing", 0);
        return kz_bool(true);
    }
    if (strcmp(op, "rtl") == 0) {
        len = kz_mav_command_long(payload, KZ_MAV_CMD_NAV_RETURN_TO_LAUNCH, 1, 1, 0, 0, 0, 0, 0, 0, 0);
        kz_sitl_send(KZ_MAV_MSG_COMMAND_LONG, payload, len);
        kz_sitl_wait(kz_sitl_wait_disarmed, "the return to launch", 0);
        return kz_bool(true);
    }
    if (strcmp(op, "hold") == 0 || strcmp(op, "stop") == 0) {
        len = kz_mav_command_long(payload, KZ_MAV_CMD_DO_PAUSE_CONTINUE, 1, 1, 0, 0, 0, 0, 0, 0, 0);
        kz_sitl_send(KZ_MAV_MSG_COMMAND_LONG, payload, len);
        return kz_bool(true);
    }
    if (strcmp(op, "telemetry") == 0) {
        KzValue out = kz_nil();
        if (n > 0 && args[0].kind == KZ_VAL_STR) kz_sitl_telemetry_answer(args[0].as.str.data, &out);
        return out;
    }
    fprintf(stderr, "E_KAZE_CAP_OP: the SITL link does not implement drone.%s\n", op);
    exit(70);
}
