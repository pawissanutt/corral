/* Process-local realtime fixture, never a system-clock change. */
#define _GNU_SOURCE
#include <dlfcn.h>
#include <stdlib.h>
#include <time.h>

int clock_gettime(clockid_t id, struct timespec *value) {
    const char *seconds = getenv("SPRINT_FIXTURE_SECONDS");
    if (id == CLOCK_REALTIME && seconds != NULL) {
        value->tv_sec = strtoll(seconds, NULL, 10);
        value->tv_nsec = 0;
        return 0;
    }
    int (*original)(clockid_t, struct timespec *) =
        dlsym(RTLD_NEXT, "clock_gettime");
    if (original == NULL)
        abort();
    return original(id, value);
}
