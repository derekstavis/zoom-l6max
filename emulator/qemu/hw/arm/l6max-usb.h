/* SPDX-License-Identifier: GPL-2.0-or-later */
/* ChipIdea USB device-side DMA and an inherited host-token socket.
 * GPL-2.0-or-later. Host requests do not bypass firmware USB handlers.
 */
#ifndef L6MAX_USB_H
#define L6MAX_USB_H
#define L6_USB_MAX 16384
#define L6_USB_MAGIC 0x4c365553u

typedef struct L6USB {
    uint32_t regs[0x200 / 4];
    AddressSpace *as;
    qemu_irq irq;
    int fd;
    uint8_t rx[20 + L6_USB_MAX];
    unsigned used;
    GByteArray *tx;
    bool connected;
    QEMUTimer *sof_timer;
    unsigned log_count;
} L6USB;

static void l6_usb_read_host(void *opaque);
static void l6_usb_write_host(void *opaque);
static void l6_usb_handlers(L6USB *u)
{
    if (u->fd >= 0) {
        qemu_set_fd_handler(u->fd, l6_usb_read_host,
                           u->tx->len ? l6_usb_write_host : NULL, u);
    }
}
static void l6_usb_irq(L6USB *u)
{
    uint32_t otg = u->regs[0x1a4 / 4];
    qemu_set_irq(u->irq, !!((u->regs[0x144 / 4] & u->regs[0x148 / 4]) |
                            (otg & (otg >> 8) & 0x7f0000u)));
}
static void l6_usb_sof(void *opaque)
{
    L6USB *u = opaque;
    if (u->connected && (u->regs[0x140 / 4] & 1)) {
        u->regs[0x144 / 4] |= 0x80; /* SOF received */
        l6_usb_irq(u);
    }
    timer_mod(u->sof_timer, qemu_clock_get_ns(QEMU_CLOCK_VIRTUAL) + 1000000);
}
static void l6_usb_reply(L6USB *u, uint32_t seq, int status,
                         const uint8_t *data, unsigned size)
{
    uint8_t header[20];
    if (u->tx->len > 1024 * 1024) { return; }
    stl_le_p(header, L6_USB_MAGIC);
    stl_le_p(header + 4, 0x80000000u);
    stl_le_p(header + 8, seq);
    stl_le_p(header + 12, status);
    stl_le_p(header + 16, size);
    g_byte_array_append(u->tx, header, sizeof(header));
    if (size) { g_byte_array_append(u->tx, data, size); }
    l6_usb_handlers(u);
}
static bool l6_usb_dma(L6USB *u, uint32_t addr, void *data, unsigned size, bool write)
{
    return (write ? address_space_write(u->as, addr, MEMTXATTRS_UNSPECIFIED, data, size)
                  : address_space_read(u->as, addr, MEMTXATTRS_UNSPECIFIED, data, size)) == MEMTX_OK;
}
static void l6_usb_token(L6USB *u, uint32_t seq, uint32_t ep,
                         uint8_t *payload, unsigned size, bool in)
{
    unsigned n = ep & 15;
    uint32_t bit = 1u << (n + (in ? 16 : 0));
    uint32_t qh_addr = (u->regs[0x158 / 4] & ~0x7ffu) + (2 * n + in) * 64;
    uint8_t qh[64], td[32];
    if (n < 8 && (u->regs[(0x1c0 + 4 * n) / 4] & (in ? 0x10000u : 1u))) {
        l6_usb_reply(u, seq, -EPIPE, NULL, 0); return;
    }
    if (n >= 8 || !(u->regs[0x1b8 / 4] & bit)) {
        l6_usb_reply(u, seq, -EAGAIN, NULL, 0); return;
    }
    if (!l6_usb_dma(u, qh_addr, qh, sizeof(qh), false)) {
        l6_usb_reply(u, seq, -EFAULT, NULL, 0); return;
    }
    uint32_t addr = ldl_le_p(qh + 8);
    if (addr & 1) { l6_usb_reply(u, seq, -EAGAIN, NULL, 0); return; }
    addr &= ~0x1fu;
    if (!l6_usb_dma(u, addr, td, sizeof(td), false)) {
        l6_usb_reply(u, seq, -EFAULT, NULL, 0); return;
    }
    uint32_t token = ldl_le_p(td + 4);
    unsigned length = (token >> 16) & 0x7fff;
    if (!(token & 0x80)) { l6_usb_reply(u, seq, -EAGAIN, NULL, 0); return; }
    /* A socket transaction consumes one complete dTD or one short OUT stage.
     * Full-packet partial dTDs require buffer-offset tracking, so reject them.
     */
    unsigned packet = (ldl_le_p(qh) >> 16) & 0x7ff;
    if ((in ? length > size : size > length) || length > L6_USB_MAX) {
        l6_usb_reply(u, seq, -EMSGSIZE, NULL, 0); return;
    }
    if (!in && size && size < length && packet && size % packet == 0) {
        l6_usb_reply(u, seq, -EMSGSIZE, NULL, 0); return;
    }
    unsigned count = in ? length : MIN(length, size);
    unsigned copied = 0;
    for (unsigned page = 0; copied < count && page < 5; page++) {
        uint32_t buffer = ldl_le_p(td + 8 + page * 4);
        if (page) { buffer &= ~0xfffu; }
        unsigned chunk = MIN(count - copied, 4096u - (buffer & 4095));
        if (!l6_usb_dma(u, buffer, payload + copied, chunk, !in)) {
            l6_usb_reply(u, seq, -EFAULT, NULL, 0); return;
        }
        copied += chunk;
    }
    if (copied != count) { l6_usb_reply(u, seq, -EFAULT, NULL, 0); return; }
    token = (token & ~0x7fff0080u) | ((length - count) << 16);
    stl_le_p(td + 4, token);
    stl_le_p(qh + 4, addr);
    stl_le_p(qh + 8, ldl_le_p(td));
    memcpy(qh + 12, td + 4, 24);
    if (!l6_usb_dma(u, addr, td, sizeof(td), true) ||
        !l6_usb_dma(u, qh_addr, qh, sizeof(qh), true)) {
        l6_usb_reply(u, seq, -EFAULT, NULL, 0); return;
    }
    uint32_t next = ldl_le_p(td);
    uint8_t next_td[32];
    if ((next & 1) || (next & ~0x1fu) == addr ||
        !l6_usb_dma(u, next & ~0x1fu, next_td, sizeof(next_td), false) ||
        !(ldl_le_p(next_td + 4) & 0x80)) {
        u->regs[0x1b8 / 4] &= ~bit;
    }
    u->regs[0x1bc / 4] |= bit;
    if (token & 0x8000) { u->regs[0x144 / 4] |= 1; }
    l6_usb_irq(u);
    l6_usb_reply(u, seq, 0, in ? payload : NULL, in ? count : 0);
}
static void l6_usb_request(L6USB *u)
{
    uint32_t kind = ldl_le_p(u->rx + 4), seq = ldl_le_p(u->rx + 8);
    uint32_t ep = ldl_le_p(u->rx + 12), size = ldl_le_p(u->rx + 16);
    uint8_t *data = u->rx + 20;
    if (kind == 1 && size == 0 && ep <= 1) {
        u->connected = ep;
        u->regs[0x184 / 4] = ep ? 0x08000205 : 0;
        u->regs[0x1a4 / 4] = (u->regs[0x1a4 / 4] & ~0x1f00u) |
                              (ep ? 0xf00u : 0x1100u) | 0xc0000u;
        u->regs[0x144 / 4] |= ep ? 0x44 : 4;
        l6_usb_irq(u);
        l6_usb_reply(u, seq, 0, NULL, 0);
    } else if (!u->connected || !(u->regs[0x140 / 4] & 1)) {
        l6_usb_reply(u, seq, -ENODEV, NULL, 0);
    } else if (kind == 2 && size == 8 && ep == 0) {
        uint32_t qh = u->regs[0x158 / 4] & ~0x7ffu;
        u->regs[0x1b8 / 4] &= ~0x10001u;
        u->regs[0x1c0 / 4] &= ~0x10001u; /* SETUP clears EP0 stalls */
        if (l6_usb_dma(u, qh + 40, data, 8, true)) {
            u->regs[0x1ac / 4] |= 1;
            u->regs[0x144 / 4] |= 1;
            l6_usb_irq(u);
            l6_usb_reply(u, seq, 0, NULL, 0);
        } else { l6_usb_reply(u, seq, -EFAULT, NULL, 0); }
    } else if ((kind == 3 || kind == 4) && (ep & ~0x8fu) == 0) {
        uint8_t payload[L6_USB_MAX];
        if (kind == 4) { memcpy(payload, data, size); }
        l6_usb_token(u, seq, ep, payload, size, kind == 3);
    } else { l6_usb_reply(u, seq, -EINVAL, NULL, 0); }
}
static void l6_usb_read_host(void *opaque)
{
    L6USB *u = opaque;
    ssize_t n = read(u->fd, u->rx + u->used, sizeof(u->rx) - u->used);
    if (n <= 0) {
        if (n == 0 || (errno != EAGAIN && errno != EINTR)) {
            qemu_set_fd_handler(u->fd, NULL, NULL, NULL); close(u->fd); u->fd = -1;
            u->connected = false; u->regs[0x184 / 4] = 0;
        }
        return;
    }
    u->used += n;
    while (u->used >= 20) {
        unsigned size = ldl_le_p(u->rx + 16);
        unsigned kind = ldl_le_p(u->rx + 4);
        unsigned wire = 20 + ((kind == 2 || kind == 4) ? size : 0);
        if (ldl_le_p(u->rx) != L6_USB_MAGIC || size > L6_USB_MAX || !ldl_le_p(u->rx + 8)) {
            qemu_set_fd_handler(u->fd, NULL, NULL, NULL); close(u->fd); u->fd = -1; return;
        }
        if (u->used < wire) { return; }
        l6_usb_request(u);
        memmove(u->rx, u->rx + wire, u->used - wire); u->used -= wire;
    }
}
static void l6_usb_write_host(void *opaque)
{
    L6USB *u = opaque;
    ssize_t n = write(u->fd, u->tx->data, u->tx->len);
    if (n > 0) { g_byte_array_remove_range(u->tx, 0, n); }
    else if (n < 0 && errno != EAGAIN && errno != EINTR) {
        qemu_set_fd_handler(u->fd, NULL, NULL, NULL); close(u->fd); u->fd = -1;
    }
    l6_usb_handlers(u);
}
static uint64_t l6_usb_read(void *opaque, hwaddr offset, unsigned size)
{
    L6USB *u = opaque;
    if (offset == 0x100) { return 0x01000040; }
    if (offset == 0x120) { return 1; }
    if (offset == 0x124) { return 0x88; } /* eight device endpoints */
    if (offset == 0x14c) {
        return (qemu_clock_get_ns(QEMU_CLOCK_VIRTUAL) / 125000) & 0x3fff;
    }
    return u->regs[offset / 4];
}
static void l6_usb_write(void *opaque, hwaddr offset, uint64_t value, unsigned size)
{
    L6USB *u = opaque;
    uint32_t *r = &u->regs[offset / 4];
    if (u->fd >= 0 && u->log_count++ < 4096) {
        qemu_log_mask(LOG_GUEST_ERROR, "l6_usb_write offset=0x%03" HWADDR_PRIx " value=0x%08" PRIx64 "\n", offset, value);
    }
    if (offset == 0x140 && (value & 2)) {
        memset(u->regs, 0, sizeof(u->regs));
        u->regs[0x184 / 4] = u->connected ? 0x08000205 : 0;
        u->regs[0x1a4 / 4] = u->connected ? 0xf00u : 0x1100u;
    } else if (offset == 0x144 || offset == 0x1ac || offset == 0x1bc) {
        *r &= ~value;
    } else if (offset == 0x1b0) {
        u->regs[0x1b8 / 4] |= value; *r = 0;
    } else if (offset == 0x1b4) {
        u->regs[0x1b8 / 4] &= ~value; *r = 0;
    } else if (offset == 0x1a4) {
        /* OTG status is read-only; interrupt flags are W1C. */
        *r = (*r & 0x0000ff00u) | (*r & ~value & 0x007f0000u) |
             (value & 0x7f0000ffu);
    } else if (offset != 0x1b8 && offset != 0x184) { *r = value; }
    l6_usb_irq(u);
}
static const MemoryRegionOps l6_usb_ops = {
    .read = l6_usb_read, .write = l6_usb_write,
    .endianness = DEVICE_LITTLE_ENDIAN,
    .valid = { .min_access_size = 4, .max_access_size = 4 },
};
#endif
