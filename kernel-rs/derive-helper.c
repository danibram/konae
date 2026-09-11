/* Setuid-root helper. Drops to littlebox-derive before the model runs, so a
   model that escapes the syntax check still cannot read /data (mode 0700,
   owned by littlebox) or setuid back to the kernel user. */
#define _GNU_SOURCE
#include <grp.h>
#include <sys/stat.h>
#include <sys/types.h>
#include <unistd.h>

#ifndef DERIVE_UID
#define DERIVE_UID 10002
#endif
#ifndef DERIVE_GID
#define DERIVE_GID 10002
#endif

int main(void) {
    char *envp[] = {"HOME=/tmp", "PATH=/usr/local/bin:/usr/bin", "TMPDIR=/tmp", NULL};
    umask(0077);
    if (setgroups(0, NULL) != 0) return 127;
    if (setgid(DERIVE_GID) != 0) return 127;
    if (setuid(DERIVE_UID) != 0) return 127;
    if (geteuid() != DERIVE_UID || getuid() != DERIVE_UID) return 127;
    execle("/usr/local/bin/bun", "bun", "run",
           "/opt/littlebox/database-lifecycle/derive-migration.ts", (char *)NULL, envp);
    return 127;
}
