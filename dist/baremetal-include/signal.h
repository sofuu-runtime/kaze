/* bare-metal stub: signals do not exist on a board, and the runtime's cleanup handler is
 * compiled out with KAZE_NO_SIGNALS. These keep an emitted program that mentions them
 * linkable without pretending anything happens. */
#ifndef KAZE_BAREMETAL_SIGNAL_H
#define KAZE_BAREMETAL_SIGNAL_H

#define SIGINT 2
#define SIGTERM 15
#define SIGSEGV 11
#define SIG_DFL ((void (*)(int))0)

typedef void (*kaze_sighandler_t)(int);
kaze_sighandler_t signal(int sig, kaze_sighandler_t handler);
int raise(int sig);

#endif
