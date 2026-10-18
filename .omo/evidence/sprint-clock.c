/* Process-local realtime fixture, never a system-clock change. */
#define _GNU_SOURCE
#include <dlfcn.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>

int clock_gettime(clockid_t id, struct timespec *value) {
    const char *seconds = getenv("SPRINT_FIXTURE_SECONDS");
    if (id == CLOCK_REALTIME && seconds != NULL) {
        const char *review_seconds = getenv("SPRINT_FIXTURE_REVIEW_SECONDS");
        if (review_seconds != NULL) {
            FILE *command = fopen("/proc/self/cmdline", "rb");
            if (command == NULL)
                abort();
            char argument[4096];
            size_t used = 0;
            int byte;
            while ((byte = fgetc(command)) != EOF) {
                if (byte == 0) {
                    argument[used] = 0;
                    if (strcmp(argument, "review-sprint") == 0)
                        seconds = review_seconds;
                    used = 0;
                } else if (used + 1 < sizeof(argument)) {
                    argument[used++] = (char)byte;
                }
            }
            fclose(command);
        }
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
