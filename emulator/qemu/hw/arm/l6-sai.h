/* SPDX-License-Identifier: GPL-2.0-or-later */
/* Shared register and FIFO behavior for the RT SAI blocks used by L6 models. */
#ifndef HW_ARM_L6_SAI_H
#define HW_ARM_L6_SAI_H

#include <stdbool.h>
#include <stdint.h>
#include <stddef.h>
#include <string.h>

/* NXP RT1041 SDK: FSL_FEATURE_SAI_FIFO_COUNTn(x) == 32. */
#define L6_SAI_FIFO_DEPTH 32
#define L6_SAI_CHANNELS 4

#define L6_SAI_VERID 0x00
#define L6_SAI_PARAM 0x04
#define L6_SAI_VERID_RESET 0x03000000u
#define L6_SAI_PARAM_SAI1_RESET 0x00050504u
#define L6_SAI_PARAM_SAI23_RESET 0x00050501u
#define L6_SAI_TCSR 0x08
#define L6_SAI_TCR1 0x0c
#define L6_SAI_TCR2 0x10
#define L6_SAI_TCR3 0x14
#define L6_SAI_TCR4 0x18
#define L6_SAI_TCR5 0x1c
#define L6_SAI_TDR0 0x20
#define L6_SAI_TFR0 0x40
#define L6_SAI_TMR 0x60
#define L6_SAI_RCSR 0x88
#define L6_SAI_RCR1 0x8c
#define L6_SAI_RCR2 0x90
#define L6_SAI_RCR3 0x94
#define L6_SAI_RCR4 0x98
#define L6_SAI_RCR5 0x9c
#define L6_SAI_RDR0 0xa0
#define L6_SAI_RFR0 0xc0
#define L6_SAI_RMR 0xe0

#define L6_SAI_TE      (1u << 31)
#define L6_SAI_RE      (1u << 31)
#define L6_SAI_FR      (1u << 25)
#define L6_SAI_SR      (1u << 24)
#define L6_SAI_WSF     (1u << 20)
#define L6_SAI_SEF     (1u << 19)
#define L6_SAI_FEF     (1u << 18)
#define L6_SAI_FWF     (1u << 17)
#define L6_SAI_FRF     (1u << 16)
#define L6_SAI_FEIE    (1u << 10)
#define L6_SAI_FWIE    (1u << 9)
#define L6_SAI_FRIE    (1u << 8)
#define L6_SAI_FRDE    (1u << 0)
#define L6_SAI_BCD     (1u << 24)
#define L6_SAI_DIV_MASK 0xffu
#define L6_SAI_FRSZ_SHIFT 16
#define L6_SAI_FRSZ_MASK  (0x1fu << L6_SAI_FRSZ_SHIFT)
#define L6_SAI_W0W_SHIFT 16
#define L6_SAI_W0W_MASK (0x1fu << L6_SAI_W0W_SHIFT)
#define L6_SAI_FCOMB_SHIFT 26
#define L6_SAI_FCOMB_MASK (3u << L6_SAI_FCOMB_SHIFT)
#define L6_SAI_RX_FCOMB_WRITE (1u << L6_SAI_FCOMB_SHIFT)
#define L6_SAI_RX_FCOMB_READ (2u << L6_SAI_FCOMB_SHIFT)
#define L6_SAI_RX_FCOMB_READ_WRITE (3u << L6_SAI_FCOMB_SHIFT)

#define L6_SAI_STATUS (L6_SAI_WSF | L6_SAI_SEF | L6_SAI_FEF | \
                       L6_SAI_FWF | L6_SAI_FRF)
#define L6_SAI_W1C    (L6_SAI_WSF | L6_SAI_SEF | L6_SAI_FEF)
#define L6_SAI_RO     (L6_SAI_FWF | L6_SAI_FRF)

typedef struct L6Sai {
    uint32_t regs[0x100 / sizeof(uint32_t)];
    /* RT1041 implements FIFO-combine fields on SAI1 only; SAI2/SAI3
     * reserve these bits. */
    bool supports_combine;
    uint32_t tx_fifo[L6_SAI_CHANNELS][L6_SAI_FIFO_DEPTH];
    uint32_t rx_fifo[L6_SAI_CHANNELS][L6_SAI_FIFO_DEPTH];
    unsigned tx_read[L6_SAI_CHANNELS];
    unsigned tx_write[L6_SAI_CHANNELS];
    unsigned tx_count[L6_SAI_CHANNELS];
    unsigned rx_read[L6_SAI_CHANNELS];
    unsigned rx_write[L6_SAI_CHANNELS];
    unsigned rx_count[L6_SAI_CHANNELS];
    unsigned rx_combine_next;
} L6Sai;

static inline void l6_sai_init(L6Sai *s, bool supports_combine)
{
    memset(s, 0, sizeof(*s));
    s->supports_combine = supports_combine;
    s->regs[L6_SAI_VERID / 4] = L6_SAI_VERID_RESET;
    s->regs[L6_SAI_PARAM / 4] = supports_combine ?
                                L6_SAI_PARAM_SAI1_RESET :
                                L6_SAI_PARAM_SAI23_RESET;
}

static inline void l6_sai_fifo_reset(L6Sai *s, bool tx)
{
    if (tx) {
        memset(s->tx_read, 0, sizeof(s->tx_read));
        memset(s->tx_write, 0, sizeof(s->tx_write));
        memset(s->tx_count, 0, sizeof(s->tx_count));
    } else {
        memset(s->rx_read, 0, sizeof(s->rx_read));
        memset(s->rx_write, 0, sizeof(s->rx_write));
        memset(s->rx_count, 0, sizeof(s->rx_count));
        s->rx_combine_next = 0;
    }
}

static inline uint32_t l6_sai_channel_mask(L6Sai *s, bool tx)
{
    uint32_t offset = tx ? L6_SAI_TCR3 : L6_SAI_RCR3;
    return (s->regs[offset / 4] >> 16) & ((1u << L6_SAI_CHANNELS) - 1);
}

static inline uint32_t l6_sai_status(L6Sai *s, bool tx)
{
    uint32_t control_offset = tx ? L6_SAI_TCSR : L6_SAI_RCSR;
    uint32_t watermark_offset = tx ? L6_SAI_TCR1 : L6_SAI_RCR1;
    uint32_t watermark = s->regs[watermark_offset / 4] & 0x1f;
    uint32_t status = s->regs[control_offset / 4] & ~L6_SAI_RO;
    uint32_t enabled = l6_sai_channel_mask(s, tx);

    /* The request is the OR across enabled data-line FIFOs. TX requests when
     * any FIFO is at/below its watermark; RX requests when any FIFO is above
     * its watermark. Warning flags similarly aggregate empty/full FIFOs. */
    for (unsigned channel = 0; channel < L6_SAI_CHANNELS; channel++) {
        if (!(enabled & (1u << channel))) {
            continue;
        }
        unsigned count = tx ? s->tx_count[channel] : s->rx_count[channel];
        if ((tx && count <= watermark) || (!tx && count > watermark)) {
            status |= L6_SAI_FRF;
        }
        if ((tx && count == 0) || (!tx && count == L6_SAI_FIFO_DEPTH)) {
            status |= L6_SAI_FWF;
        }
    }
    s->regs[control_offset / 4] = status;
    return status;
}

static inline unsigned l6_sai_rx_read_channel(L6Sai *s, unsigned requested)
{
    uint32_t enabled = l6_sai_channel_mask(s, false);
    uint32_t combine = s->regs[L6_SAI_RCR4 / 4] & L6_SAI_FCOMB_MASK;
    if (!s->supports_combine || (combine != L6_SAI_RX_FCOMB_READ &&
         combine != L6_SAI_RX_FCOMB_READ_WRITE) || !enabled) {
        return requested;
    }
    for (unsigned step = 0; step < L6_SAI_CHANNELS; step++) {
        unsigned channel = (s->rx_combine_next + step) % L6_SAI_CHANNELS;
        if (enabled & (1u << channel)) {
            s->rx_combine_next = (channel + 1) % L6_SAI_CHANNELS;
            return channel;
        }
    }
    return requested;
}

static inline unsigned l6_sai_frame_words(L6Sai *s, bool tx)
{
    uint32_t offset = tx ? L6_SAI_TCR4 : L6_SAI_RCR4;
    return ((s->regs[offset / 4] & L6_SAI_FRSZ_MASK) >>
            L6_SAI_FRSZ_SHIFT) + 1;
}

/* TCR2.DIV divides the source by 2 * (DIV + 1). FRSZ and W0W
 * encode the frame slots and first-word width as value minus one. */
static inline unsigned l6_sai_sample_rate(uint32_t source_hz, uint32_t cr2,
                                          uint32_t cr4, uint32_t cr5)
{
    unsigned slots = ((cr4 & L6_SAI_FRSZ_MASK) >> L6_SAI_FRSZ_SHIFT) + 1;
    unsigned width = ((cr5 & L6_SAI_W0W_MASK) >> L6_SAI_W0W_SHIFT) + 1;
    unsigned divider = ((cr2 & L6_SAI_DIV_MASK) + 1) * 2;
    uint64_t bitclocks_per_frame = (uint64_t)divider * slots * width;

    if (!source_hz || !bitclocks_per_frame) {
        return 0;
    }
    return source_hz / bitclocks_per_frame;
}

/* Advance one sample frame on the externally supplied or internally
 * generated SAI clocks. Missing TX words produce silence; the caller can
 * forward the returned frame to a QEMU audio backend. */
static inline unsigned l6_sai_tx_frame(L6Sai *s, uint32_t *samples,
                                        unsigned capacity)
{
    unsigned words = l6_sai_frame_words(s, true);
    if (words > capacity) {
        words = capacity;
    }
    uint32_t enabled = l6_sai_channel_mask(s, true);
    uint32_t masked_words = s->regs[L6_SAI_TMR / 4];
    memset(samples, 0, words * sizeof(*samples));
    for (unsigned channel = 0; channel < L6_SAI_CHANNELS; channel++) {
        if (!(enabled & (1u << channel))) {
            continue;
        }
        unsigned channel_words = words;
        for (unsigned i = 0; i < channel_words; i++) {
            /* TMR masks frame words before the transmitter reads its FIFO.
             * Advancing the FIFO for masked slots shifts every later sample
             * and corrupts TDM streams whose payload starts mid-frame. */
            if (masked_words & (1u << i)) {
                continue;
            }
            if (s->tx_count[channel]) {
                uint32_t sample = s->tx_fifo[channel][s->tx_read[channel]];
                s->tx_read[channel] = (s->tx_read[channel] + 1) %
                                      L6_SAI_FIFO_DEPTH;
                s->tx_count[channel]--;
                if (channel == 0) {
                    samples[i] = sample;
                }
            } else {
                /* An enabled TX FIFO underrun transmits silence. */
                s->regs[L6_SAI_TCSR / 4] |= L6_SAI_FEF;
            }
        }
    }
    l6_sai_status(s, true);
    return words;
}

static inline void l6_sai_rx_frame(L6Sai *s, const uint32_t *samples,
                                    unsigned words)
{
    uint32_t enabled = l6_sai_channel_mask(s, false);
    uint32_t combine = s->regs[L6_SAI_RCR4 / 4] & L6_SAI_FCOMB_MASK;
    bool combine_write = s->supports_combine &&
        (combine == L6_SAI_RX_FCOMB_WRITE ||
         combine == L6_SAI_RX_FCOMB_READ_WRITE);

    /* RX FCOMB=read changes software FIFO reads only: the receive shifter
     * still writes each data line's words to that line's FIFO. FCOMB=write
     * instead distributes successive unmasked words across enabled FIFOs. */
    if (combine_write) {
        if (!enabled) {
            l6_sai_status(s, false);
            return;
        }
        unsigned channel = 0;
        while (channel < L6_SAI_CHANNELS && !(enabled & (1u << channel))) {
            channel++;
        }
        for (unsigned i = 0; i < words && channel < L6_SAI_CHANNELS; i++) {
            if (s->rx_count[channel] == L6_SAI_FIFO_DEPTH) {
                s->regs[L6_SAI_RCSR / 4] |= L6_SAI_FEF;
            } else {
                s->rx_fifo[channel][s->rx_write[channel]] =
                    samples ? samples[i] : 0;
                s->rx_write[channel] = (s->rx_write[channel] + 1) %
                                       L6_SAI_FIFO_DEPTH;
                s->rx_count[channel]++;
            }
            unsigned next = (channel + 1) % L6_SAI_CHANNELS;
            while (!(enabled & (1u << next))) {
                next = (next + 1) % L6_SAI_CHANNELS;
            }
            channel = next;
        }
    } else {
        for (unsigned channel = 0; channel < L6_SAI_CHANNELS; channel++) {
            if (!(enabled & (1u << channel))) {
                continue;
            }
            for (unsigned i = 0; i < words; i++) {
                if (s->rx_count[channel] == L6_SAI_FIFO_DEPTH) {
                    s->regs[L6_SAI_RCSR / 4] |= L6_SAI_FEF;
                    break;
                }
                s->rx_fifo[channel][s->rx_write[channel]] =
                    samples ? samples[channel * words + i] : 0;
                s->rx_write[channel] = (s->rx_write[channel] + 1) %
                                       L6_SAI_FIFO_DEPTH;
                s->rx_count[channel]++;
            }
        }
    }
    l6_sai_status(s, false);
}

static inline uint32_t l6_sai_read(L6Sai *s, uint32_t offset, unsigned size)
{
    uint32_t aligned = offset & ~3u;
    unsigned shift = (offset & 3) * 8;
    uint32_t word;

    if (aligned >= L6_SAI_TDR0 &&
        aligned < L6_SAI_TDR0 + 4 * L6_SAI_CHANNELS) {
        word = 0;
    } else if (aligned >= L6_SAI_RDR0 &&
               aligned < L6_SAI_RDR0 + 4 * L6_SAI_CHANNELS) {
        unsigned requested = (aligned - L6_SAI_RDR0) / 4;
        unsigned channel = l6_sai_rx_read_channel(s, requested);
        if (channel >= L6_SAI_CHANNELS || !s->rx_count[channel]) {
            word = 0;
        } else {
            word = s->rx_fifo[channel][s->rx_read[channel]];
            s->rx_read[channel] = (s->rx_read[channel] + 1) %
                                  L6_SAI_FIFO_DEPTH;
            s->rx_count[channel]--;
            l6_sai_status(s, false);
        }
        return size == 4 ? word : (word >> shift) & ((1u << (size * 8)) - 1);
    } else if (aligned >= L6_SAI_TFR0 &&
               aligned < L6_SAI_TFR0 + 4 * L6_SAI_CHANNELS) {
        unsigned channel = (aligned - L6_SAI_TFR0) / 4;
        word = (s->tx_write[channel] << 16) | s->tx_read[channel];
    } else if (aligned >= L6_SAI_RFR0 &&
               aligned < L6_SAI_RFR0 + 4 * L6_SAI_CHANNELS) {
        unsigned channel = (aligned - L6_SAI_RFR0) / 4;
        word = (s->rx_write[channel] << 16) | s->rx_read[channel];
    } else {
        switch (aligned) {
        case L6_SAI_TCSR:
            word = l6_sai_status(s, true);
            break;
        case L6_SAI_RCSR:
            word = l6_sai_status(s, false);
            break;
        default:
            word = aligned < sizeof(s->regs) ? s->regs[aligned / 4] : 0;
            break;
        }
    }
    return size == 4 ? word : (word >> shift) & ((1u << (size * 8)) - 1);
}

static inline void l6_sai_write(L6Sai *s, uint32_t offset, uint64_t value,
                                unsigned size)
{
    uint32_t aligned = offset & ~3u;
    unsigned shift = (offset & 3) * 8;
    uint32_t old = aligned < sizeof(s->regs) ? s->regs[aligned / 4] : 0;
    uint32_t mask = size == 4 ? UINT32_MAX :
                    ((1u << (size * 8)) - 1) << shift;
    uint32_t write_bits = ((uint32_t)value << shift) & mask;
    uint32_t merged = (old & ~mask) | write_bits;

    if (aligned >= L6_SAI_TDR0 &&
        aligned < L6_SAI_TDR0 + 4 * L6_SAI_CHANNELS) {
        unsigned channel = (aligned - L6_SAI_TDR0) / 4;
        if ((l6_sai_channel_mask(s, true) & (1u << channel)) &&
            s->tx_count[channel] < L6_SAI_FIFO_DEPTH) {
            s->tx_fifo[channel][s->tx_write[channel]] = (uint32_t)value;
            s->tx_write[channel] = (s->tx_write[channel] + 1) %
                                   L6_SAI_FIFO_DEPTH;
            s->tx_count[channel]++;
        }
        l6_sai_status(s, true);
        return;
    }
    if ((aligned >= L6_SAI_RDR0 &&
         aligned < L6_SAI_RDR0 + 4 * L6_SAI_CHANNELS) ||
        (aligned >= L6_SAI_TFR0 &&
         aligned < L6_SAI_TFR0 + 4 * L6_SAI_CHANNELS) ||
        (aligned >= L6_SAI_RFR0 &&
         aligned < L6_SAI_RFR0 + 4 * L6_SAI_CHANNELS)) {
        return;
    }

    switch (aligned) {
    case L6_SAI_TCSR:
    case L6_SAI_RCSR: {
        bool tx = aligned == L6_SAI_TCSR;
        uint32_t old = l6_sai_status(s, tx);
        if (merged & L6_SAI_FR) {
            l6_sai_fifo_reset(s, tx);
        }
        merged &= ~(L6_SAI_FR | L6_SAI_SR);
        merged = (merged & ~L6_SAI_STATUS) | (old & L6_SAI_RO) |
                 (old & L6_SAI_W1C & ~write_bits);
        s->regs[aligned / 4] = merged;
        l6_sai_status(s, tx);
        return;
    }
    default:
        if (aligned < sizeof(s->regs)) {
            if (!s->supports_combine &&
                (aligned == L6_SAI_TCR4 || aligned == L6_SAI_RCR4)) {
                merged &= ~L6_SAI_FCOMB_MASK;
            }
            s->regs[aligned / 4] = merged;
        }
    }
}

#endif /* HW_ARM_L6_SAI_H */
