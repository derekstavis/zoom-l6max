/* SPDX-License-Identifier: GPL-2.0-or-later */
/* Narrow 128x64 OLED model for the L6max's observed SPI command stream.
 * Copyright (c) 2026. Licensed under GPL-2.0-or-later.
 */
#ifndef HW_ARM_L6MAX_DISPLAY_H
#define HW_ARM_L6MAX_DISPLAY_H

#include "qemu/osdep.h"
#include "qemu/atomic.h"
#include "qapi/error.h"
#include <fcntl.h>
#include <sys/mman.h>
#include <sys/stat.h>
#include <unistd.h>

#define L6_DISPLAY_PAGE_BYTES 128
#define L6_DISPLAY_PAGES 8
#define L6_DISPLAY_FRAME_BYTES (L6_DISPLAY_PAGE_BYTES * L6_DISPLAY_PAGES)
#define L6_DISPLAY_SHARED_BYTES (2 * L6_DISPLAY_FRAME_BYTES)

typedef void (*L6DisplayPublish)(void *opaque, uint32_t generation,
                                 uint32_t slot, uint32_t dirty_pages);

typedef struct L6Display {
    uint8_t ram[L6_DISPLAY_FRAME_BYTES];
    uint8_t completed[L6_DISPLAY_FRAME_BYTES];
    uint8_t published[L6_DISPLAY_FRAME_BYTES];
    uint8_t *shared;
    int fd;
    L6DisplayPublish publish;
    void *opaque;
    uint32_t generation;
    uint32_t slot_generation[2];
    bool busy[2];
    bool on;
    bool entire_on;
    bool inverse;
    bool dirty;
    bool pending;
    uint8_t page;
    uint8_t column;
    uint8_t addressing_mode;
    uint8_t column_start;
    uint8_t column_end;
    uint8_t page_start;
    uint8_t page_end;
    uint8_t command;
    uint8_t parameters[6];
    uint8_t parameter_count;
    uint8_t parameters_remaining;
} L6Display;

/* On success this object owns fd. On failure the caller retains ownership.
 * fd=-1 permits the controller to run without a host display consumer. */
static bool l6_display_init(L6Display *d, int fd, L6DisplayPublish publish,
                            void *opaque, Error **errp)
{
    memset(d, 0, sizeof(*d));
    d->fd = -1;
    d->column_end = L6_DISPLAY_PAGE_BYTES - 1;
    d->page_end = L6_DISPLAY_PAGES - 1;
    d->addressing_mode = 2; /* Default page addressing, as used by firmware. */
    d->publish = publish;
    d->opaque = opaque;
    if (fd == -1) {
        return true;
    }
    struct stat st;
    int flags = fcntl(fd, F_GETFL);
    if (fd < 0 || flags < 0 || (flags & O_ACCMODE) != O_RDWR ||
        fstat(fd, &st) < 0 || st.st_size < L6_DISPLAY_SHARED_BYTES ||
        !publish) {
        error_setg(errp, "display descriptor must be writable, at least %u bytes, and have a publish callback",
                   L6_DISPLAY_SHARED_BYTES);
        return false;
    }
    void *shared = mmap(NULL, L6_DISPLAY_SHARED_BYTES,
                        PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0);
    if (shared == MAP_FAILED) {
        error_setg_errno(errp, errno, "cannot map display frames");
        return false;
    }
    d->shared = shared;
    d->fd = fd;
    memset(d->shared, 0, L6_DISPLAY_SHARED_BYTES);
    return true;
}

static uint8_t l6_display_pixel_byte(const L6Display *d, unsigned i)
{
    if (!d->on) {
        return 0;
    }
    uint8_t value = d->entire_on ? 0xff : d->ram[i];
    return d->inverse ? value ^ 0xff : value;
}

static void l6_display_publish_pending(L6Display *d)
{
    if (!d->pending) {
        return;
    }
    uint32_t dirty_pages = 0;
    for (unsigned i = 0; i < L6_DISPLAY_FRAME_BYTES; i++) {
        if (d->completed[i] != d->published[i]) {
            dirty_pages |= 1u << (i / L6_DISPLAY_PAGE_BYTES);
        }
    }
    if (!dirty_pages) {
        d->pending = false;
        return;
    }
    if (!d->shared) {
        return;
    }
    unsigned slot;
    for (slot = 0; slot < 2; slot++) {
        if (!d->busy[slot]) {
            break;
        }
    }
    if (slot == 2) {
        /* Coalesce pending transfers while the UI owns both frame slots. */
        return;
    }
    uint8_t *frame = d->shared + slot * L6_DISPLAY_FRAME_BYTES;
    memcpy(frame, d->completed, sizeof(d->completed));
    memcpy(d->published, frame, sizeof(d->published));
    d->pending = false;
    if (++d->generation == 0) {
        d->generation = 1;
    }
    d->slot_generation[slot] = d->generation;
    d->busy[slot] = true;
    /* Publish only after all pixels and ownership metadata are ready. */
    smp_wmb();
    d->publish(d->opaque, d->generation, slot, dirty_pages);
}

/* Called only after the modeled SPI transfer has actually completed. */
static void l6_display_flush(L6Display *d)
{
    if (d->dirty) {
        for (unsigned i = 0; i < L6_DISPLAY_FRAME_BYTES; i++) {
            d->completed[i] = l6_display_pixel_byte(d, i);
        }
        d->dirty = false;
        d->pending = true;
    }
    l6_display_publish_pending(d);
}

static bool l6_display_ack(L6Display *d, uint32_t generation, uint32_t slot)
{
    if (!generation || slot >= 2 || !d->busy[slot] ||
        d->slot_generation[slot] != generation) {
        return false;
    }
    d->busy[slot] = false;
    /* An ack may arrive during another DMA transfer. Publish only pixels
     * latched by the last completed transfer, never its in-flight writes. */
    l6_display_publish_pending(d);
    return true;
}

static void l6_display_parameters(L6Display *d)
{
    switch (d->command) {
    case 0x20:
        if (d->parameters[0] <= 2) {
            d->addressing_mode = d->parameters[0];
        }
        break;
    case 0x21:
        if (d->parameters[0] <= d->parameters[1] &&
            d->parameters[1] < L6_DISPLAY_PAGE_BYTES) {
            d->column_start = d->parameters[0];
            d->column_end = d->parameters[1];
            d->column = d->column_start;
        }
        break;
    case 0x22:
        if (d->parameters[0] <= d->parameters[1] &&
            d->parameters[1] < L6_DISPLAY_PAGES) {
            d->page_start = d->parameters[0];
            d->page_end = d->parameters[1];
            d->page = d->page_start;
        }
        break;
    default:
        /* Brightness, analog timing, orientation and scrolling are consumed
         * without changing the raw page layout presented to the native UI. */
        break;
    }
}

static void l6_display_command(L6Display *d, uint8_t value)
{
    if (d->parameters_remaining) {
        d->parameters[d->parameter_count++] = value;
        if (--d->parameters_remaining == 0) {
            l6_display_parameters(d);
        }
        return;
    }
    d->command = value;
    d->parameter_count = 0;
    switch (value) {
    case 0x20: case 0x81: case 0x8d: case 0xa8:
    case 0xd3: case 0xd5: case 0xd6: case 0xd9:
    case 0xda: case 0xdb: case 0xfd:
        d->parameters_remaining = 1;
        break;
    case 0x21: case 0x22: case 0xa3:
        d->parameters_remaining = 2;
        break;
    case 0x26: case 0x27:
        d->parameters_remaining = 6;
        break;
    case 0x29: case 0x2a:
        d->parameters_remaining = 5;
        break;
    case 0xae: case 0xaf:
        d->on = value == 0xaf;
        d->dirty = true;
        break;
    case 0xa4: case 0xa5:
        d->entire_on = value == 0xa5;
        d->dirty = true;
        break;
    case 0xa6: case 0xa7:
        d->inverse = value == 0xa7;
        d->dirty = true;
        break;
    default:
        if ((value & 0xf8) == 0xb0) {
            d->page = value & 7;
        } else if ((value & 0xf0) == 0x00) {
            d->column = (d->column & 0xf0) | (value & 0x0f);
        } else if ((value & 0xf0) == 0x10) {
            d->column = (d->column & 0x0f) | ((value & 0x0f) << 4);
        }
        break;
    }
}

static void l6_display_advance(L6Display *d)
{
    if (d->addressing_mode == 2) {
        d->column = (d->column + 1) % L6_DISPLAY_PAGE_BYTES;
    } else if (d->addressing_mode == 0) {
        if (d->column >= d->column_end) {
            d->column = d->column_start;
            d->page = d->page >= d->page_end ? d->page_start : d->page + 1;
        } else {
            d->column++;
        }
    } else {
        if (d->page >= d->page_end) {
            d->page = d->page_start;
            d->column = d->column >= d->column_end ?
                d->column_start : d->column + 1;
        } else {
            d->page++;
        }
    }
}

static void l6_display_bytes(L6Display *d, bool dc, const uint8_t *bytes,
                             size_t length)
{
    for (size_t i = 0; i < length; i++) {
        if (!dc) {
            l6_display_command(d, bytes[i]);
            continue;
        }
        if (d->column < L6_DISPLAY_PAGE_BYTES && d->page < L6_DISPLAY_PAGES) {
            unsigned index = d->page * L6_DISPLAY_PAGE_BYTES + d->column;
            if (d->ram[index] != bytes[i]) {
                d->ram[index] = bytes[i];
                d->dirty = true;
            }
        }
        l6_display_advance(d);
    }
}

static void l6_display_close(L6Display *d)
{
    memset(d->busy, 0, sizeof(d->busy));
    if (d->shared) {
        munmap(d->shared, L6_DISPLAY_SHARED_BYTES);
        d->shared = NULL;
    }
    if (d->fd >= 0) {
        close(d->fd);
        d->fd = -1;
    }
}

#endif /* HW_ARM_L6MAX_DISPLAY_H */
