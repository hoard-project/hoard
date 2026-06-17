// Hoard eBPF v5 — multi-hook: vfs_write + ext4_file_write_iter
// Covers both generic VFS writes (tmpfs, nfs) and ext4 direct writes.

#include "vmlinux.h"

#define SEC(name)  __attribute__((section(name), used))
#define __uint(N, V)  int (*N)[V]
#define __type(N, V)  typeof(V) *N
#define __always_inline  inline __attribute__((always_inline))

char __license[] SEC("license") = "GPL";

static void *(*bpf_map_lookup_elem)(void *map, const void *key) = (void *) 1;
static long (*bpf_map_update_elem)(void *map, const void *key, const void *value, __u64 flags) = (void *) 2;
static long (*bpf_ringbuf_reserve)(void *ringbuf, __u64 size, __u64 flags) = (void *) 131;
static void (*bpf_ringbuf_submit)(void *data, __u64 flags) = (void *) 132;
static __u64 (*bpf_ktime_get_ns)(void) = (void *) 5;

struct event { __u64 dev; __u64 ino; __u64 ts; };

struct {
    __uint(type, BPF_MAP_TYPE_RINGBUF);
    __uint(max_entries, 128 * 1024);
} events SEC(".maps");

struct {
    __uint(type, BPF_MAP_TYPE_PERCPU_ARRAY);
    __uint(max_entries, 1);
    __type(key, __u32);
    __type(value, __u64);
} diag SEC(".maps");

struct dev_ino { __u64 dev; __u64 ino; };

struct {
    __uint(type, BPF_MAP_TYPE_LRU_HASH);
    __uint(max_entries, 1024);
    __type(key, struct dev_ino);
    __type(value, __u64);
} last_emitted SEC(".maps");

#define DEDUP_NS (1ULL * 1000 * 1000)

static __always_inline void maybe_emit_from_file(struct file *file)
{
    if (!file) return;
    struct inode *inode = file->f_inode;
    if (!inode) return;
    if ((inode->i_mode & 00170000) != 0100000) return;
    struct super_block *sb = inode->i_sb;
    if (!sb) return;
    struct dev_ino key = {};
    key.ino = inode->i_ino;
    key.dev = (__u64)((__u32)sb->s_dev);
    if (key.dev == 0 && key.ino == 0) return;

    __u64 now = bpf_ktime_get_ns();
    __u64 *lt = bpf_map_lookup_elem(&last_emitted, &key);
    if (lt && (now - *lt) < DEDUP_NS) return;
    bpf_map_update_elem(&last_emitted, &key, &now, BPF_ANY);

    struct event *e = (struct event *)bpf_ringbuf_reserve(&events, sizeof(*e), 0);
    if (!e) return;
    e->dev = key.dev; e->ino = key.ino; e->ts = now;
    bpf_ringbuf_submit(e, 0);
}

static __always_inline void inc_diag(void) {
    __u32 k = 0;
    __u64 *c = bpf_map_lookup_elem(&diag, &k);
    if (c) __sync_fetch_and_add(c, 1);
}

// Hook 1: generic VFS (tmpfs, nfs, etc.)
SEC("fentry/vfs_write")
int on_vfs_write(void *ctx) {
    inc_diag();
    maybe_emit_from_file((struct file *)((unsigned long *)ctx)[0]);
    return 0;
}

// Hook 2: ext4 write_iter — file is in kiocb->ki_filp
SEC("fentry/ext4_file_write_iter")
int on_ext4_write_iter(void *ctx) {
    inc_diag();
    unsigned long *args = (unsigned long *)ctx;
    struct kiocb *iocb = (struct kiocb *)args[0];
    if (!iocb) return 0;
    maybe_emit_from_file(iocb->ki_filp);
    return 0;
}
