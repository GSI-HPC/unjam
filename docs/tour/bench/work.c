// SPDX-License-Identifier: LGPL-3.0-or-later
// SPDX-FileCopyrightText: © GSI Helmholtzzentrum f. Schwerionenforschung GmbH, Darmstadt, Germany

// Synthetic workload for the tour bench. WORK="CPU_FRACTION MEM_MB THREADS IO_KBPS".
// Start it with `exec -a NAME work ARGS...`: unjam then shows NAME ARGS as the command line
// and the first word of NAME as the process name.
#include <pthread.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
#include <sys/prctl.h>
#include <unistd.h>

static double frac;

static double now(void) {
    struct timespec t;
    clock_gettime(CLOCK_MONOTONIC, &t);
    return t.tv_sec + t.tv_nsec / 1e9;
}

static void *burn(void *arg) {
    (void)arg;
    for (;;) {
        double t = now();
        while (now() - t < 0.1 * frac) {}
        usleep((useconds_t)(1e5 * (1 - frac)));
    }
    return NULL;
}

static void *writer(void *arg) {
    long kb = (long)arg;
    char path[64], *buf = calloc(1024, (size_t)kb);
    snprintf(path, sizeof path, "/tmp/unjam-tour-work-%d", getpid());
    for (;;) {
        FILE *f = fopen(path, "wb");
        if (f) { fwrite(buf, 1024, (size_t)kb, f); fflush(f); fsync(fileno(f)); fclose(f); }
        sleep(1);
    }
    return NULL;
}

int main(int argc, char **argv) {
    (void)argc;
    char comm[16] = {0};
    sscanf(argv[0], "%15[^ :]", comm);
    prctl(PR_SET_NAME, comm);
    const char *w = getenv("WORK");
    long mem = 50, thr = 1, io = 0;
    frac = 0.1;
    if (w) sscanf(w, "%lf %ld %ld %ld", &frac, &mem, &thr, &io);
    char *blob = malloc((size_t)mem << 20);
    for (long i = 0; blob && i < mem << 20; i += 4096) blob[i] = 1;
    pthread_t t;
    for (long i = 1; i < thr; i++) pthread_create(&t, NULL, burn, NULL);
    if (io) pthread_create(&t, NULL, writer, (void *)io);
    burn(NULL);
}
