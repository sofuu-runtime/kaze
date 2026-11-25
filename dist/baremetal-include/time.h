/* bare-metal stub: a board has no wall clock, and the runtime only asks for one to
 * stamp a seed. The counter is monotonic from boot so code that reads it twice sees
 * forward motion; it is not a calendar. */
#ifndef KAZE_BAREMETAL_TIME_H
#define KAZE_BAREMETAL_TIME_H

typedef long time_t;

time_t time(time_t* out);

#endif
