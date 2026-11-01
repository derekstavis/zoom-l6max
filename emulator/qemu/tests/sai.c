#include "../hw/arm/l6-sai.h"

#include <assert.h>
#include <stdio.h>

int main(void)
{
    L6Sai sai = { 0 };
    L6Sai sai3 = { 0 };
    uint32_t status;

    /* RT1041 VERID/PARAM occupy the first words; controls are shifted while
     * the data and FIFO windows retain their familiar offsets. */
    assert(L6_SAI_TCSR == 0x08 && L6_SAI_TCR1 == 0x0c);
    assert(L6_SAI_TCR2 == 0x10 && L6_SAI_TMR == 0x60);
    assert(L6_SAI_RCSR == 0x88 && L6_SAI_RCR2 == 0x90);
    assert(L6_SAI_TDR0 == 0x20 && L6_SAI_RDR0 == 0xa0);

    l6_sai_init(&sai, true);
    l6_sai_init(&sai3, false);
    assert(l6_sai_read(&sai, L6_SAI_VERID, 4) == L6_SAI_VERID_RESET);
    assert(l6_sai_read(&sai, L6_SAI_PARAM, 4) ==
           L6_SAI_PARAM_SAI1_RESET);
    assert(l6_sai_read(&sai3, L6_SAI_VERID, 4) == L6_SAI_VERID_RESET);
    assert(l6_sai_read(&sai3, L6_SAI_PARAM, 4) ==
           L6_SAI_PARAM_SAI23_RESET);

    /* Both stock firmware images derive 48 kHz from the same nominal
     * 24.576 MHz source. L6's SAI3 uses two 32-bit slots with DIV=3; L6max's
     * SAI1 uses eight slots with DIV=0. */
    assert(l6_sai_sample_rate(24576000, 3, 1u << L6_SAI_FRSZ_SHIFT,
                              31u << L6_SAI_W0W_SHIFT) == 48000);
    assert(l6_sai_sample_rate(24576000, 0, 7u << L6_SAI_FRSZ_SHIFT,
                              31u << L6_SAI_W0W_SHIFT) == 48000);
    assert(l6_sai_sample_rate(0, 3, 1u << L6_SAI_FRSZ_SHIFT,
                              31u << L6_SAI_W0W_SHIFT) == 0);

    sai.regs[L6_SAI_TCR3 / 4] = 1u << 16;
    sai.regs[L6_SAI_RCR3 / 4] = 1u << 16;
    status = l6_sai_read(&sai, L6_SAI_TCSR, 4);
    assert((status & (L6_SAI_FRF | L6_SAI_FWF)) ==
           (L6_SAI_FRF | L6_SAI_FWF));
    assert(l6_sai_read(&sai, L6_SAI_RDR0, 4) == 0);

    uint32_t frame[8] = { 0 };
    sai.regs[L6_SAI_TCR4 / 4] = 1u << L6_SAI_FRSZ_SHIFT;
    l6_sai_write(&sai, L6_SAI_TDR0, 0x11223344, 4);
    l6_sai_write(&sai, L6_SAI_TDR0, 0x55667788, 4);
    assert(l6_sai_tx_frame(&sai, frame, 8) == 2);
    assert(frame[0] == 0x11223344 && frame[1] == 0x55667788);
    assert(sai.tx_count[0] == 0);
    l6_sai_rx_frame(&sai, frame, 2);
    assert(sai.rx_count[0] == 2);
    assert(l6_sai_read(&sai, L6_SAI_RDR0, 4) == frame[0]);

    /* RT SAI TMR-masked slots are not read from the transmit FIFO. This is
     * required for L6max's eight-slot TDM stream, whose payload begins at
     * slot 2. */
    l6_sai_init(&sai, true);
    sai.regs[L6_SAI_TCR3 / 4] = 1u << 16;
    sai.regs[L6_SAI_TCR4 / 4] = 7u << L6_SAI_FRSZ_SHIFT;
    l6_sai_write(&sai, L6_SAI_TMR, 0x3, 4);
    l6_sai_write(&sai, L6_SAI_TDR0, 0x10203040, 4);
    l6_sai_write(&sai, L6_SAI_TDR0, 0x50607080, 4);
    assert(l6_sai_tx_frame(&sai, frame, 8) == 8);
    assert(frame[0] == 0 && frame[1] == 0);
    assert(frame[2] == 0x10203040 && frame[3] == 0x50607080);
    assert(sai.tx_count[0] == 0);

    l6_sai_write(&sai, L6_SAI_TCR1, 2, 4);
    l6_sai_write(&sai, L6_SAI_TDR0, 0x11223344, 4);
    l6_sai_write(&sai, L6_SAI_TDR0, 0x55667788, 4);
    assert(sai.tx_count[0] == 2);
    assert(l6_sai_read(&sai, L6_SAI_TFR0, 4) == ((4u << 16) | 2u));
    status = l6_sai_read(&sai, L6_SAI_TCSR, 4);
    assert((status & L6_SAI_FRF) && !(status & L6_SAI_FWF));

    l6_sai_write(&sai, L6_SAI_TDR0, 0xaabbccdd, 4);
    status = l6_sai_read(&sai, L6_SAI_TCSR, 4);
    assert(!(status & (L6_SAI_FRF | L6_SAI_FWF)));

    for (unsigned i = sai.tx_count[0]; i < L6_SAI_FIFO_DEPTH; i++) {
        l6_sai_write(&sai, L6_SAI_TDR0, i, 4);
    }
    l6_sai_write(&sai, L6_SAI_TDR0, 0, 4);
    assert(sai.tx_count[0] == L6_SAI_FIFO_DEPTH);
    assert((l6_sai_read(&sai, L6_SAI_TCSR, 4) & L6_SAI_FWF) == 0);

    /* WSF and FEF are sticky W1C flags; clearing one preserves the other. */
    l6_sai_write(&sai, L6_SAI_TCSR, L6_SAI_TE, 4);
    sai.regs[L6_SAI_TCSR / 4] |= L6_SAI_WSF | L6_SAI_FEF;
    l6_sai_write(&sai, L6_SAI_TCSR, L6_SAI_TE | L6_SAI_WSF, 4);
    status = l6_sai_read(&sai, L6_SAI_TCSR, 4);
    assert(!(status & L6_SAI_WSF) && (status & L6_SAI_FEF));

    /* FIFO reset is momentary, empties the FIFO, and preserves TE. */
    l6_sai_write(&sai, L6_SAI_TCSR, L6_SAI_TE | L6_SAI_FR, 4);
    assert(sai.tx_count[0] == 0);
    assert(l6_sai_read(&sai, L6_SAI_TCSR, 4) & L6_SAI_TE);
    assert((l6_sai_read(&sai, L6_SAI_TFR0, 4) >> 16) == 0);

    /* Partial register writes merge with the stored register value. */
    l6_sai_write(&sai, L6_SAI_TCR1, 7, 1);
    assert(l6_sai_read(&sai, L6_SAI_TCR1, 4) == 7);

    /* Four enabled RX FIFOs plus FCOMB=2 make reads through RDR0 alternate
     * across the enabled data channels, as in the RT SAI RX combine mode. */
    l6_sai_init(&sai, true); /* The firmware's four-line stream is SAI1. */
    sai.regs[L6_SAI_RCR3 / 4] = 0xfu << 16;
    sai.regs[L6_SAI_RCR4 / 4] = L6_SAI_RX_FCOMB_READ;
    for (unsigned channel = 0; channel < L6_SAI_CHANNELS; channel++) {
        sai.rx_fifo[channel][0] = 0x100 + channel;
        sai.rx_write[channel] = 1;
        sai.rx_count[channel] = 1;
    }
    for (unsigned channel = 0; channel < L6_SAI_CHANNELS; channel++) {
        assert(l6_sai_read(&sai, L6_SAI_RDR0, 4) == 0x100 + channel);
    }
    for (unsigned channel = 0; channel < L6_SAI_CHANNELS; channel++) {
        assert(sai.rx_count[channel] == 0);
    }

    /* RT1041 defines FIFO combine only on SAI1; SAI3 reads its RCR4.FCOMB
     * reserved bits as zero and retains ordinary per-line FIFO behavior. */
    memset(&sai, 0, sizeof(sai));
    sai.regs[L6_SAI_RCR3 / 4] = 0x3u << 16;
    l6_sai_write(&sai, L6_SAI_RCR4, L6_SAI_RX_FCOMB_READ, 4);
    assert(!(l6_sai_read(&sai, L6_SAI_RCR4, 4) & L6_SAI_FCOMB_MASK));
    uint32_t unsupported_combine_frame[2] = { 31, 32 };
    l6_sai_rx_frame(&sai, unsupported_combine_frame, 1);
    assert(sai.rx_count[0] == 1 && sai.rx_fifo[0][0] == 31);
    assert(sai.rx_count[1] == 1 && sai.rx_fifo[1][0] == 32);

    /* RX read-combine mode still fills each enabled data-line FIFO with its
     * own frame words; reads through RDR0 then alternate across those FIFOs. */
    sai.supports_combine = true;
    sai.regs[L6_SAI_RCR3 / 4] = 0xfu << 16;
    l6_sai_fifo_reset(&sai, false);
    sai.regs[L6_SAI_RCR4 / 4] = L6_SAI_RX_FCOMB_READ | (1u << L6_SAI_FRSZ_SHIFT);
    sai.regs[L6_SAI_RCR1 / 4] = 1;
    uint32_t rx_frame[8] = { 1, 2, 3, 4, 5, 6, 7, 8 };
    l6_sai_rx_frame(&sai, rx_frame, l6_sai_frame_words(&sai, false));
    for (unsigned channel = 0; channel < L6_SAI_CHANNELS; channel++) {
        assert(sai.rx_count[channel] == 2);
        assert(sai.rx_fifo[channel][0] == channel * 2 + 1);
        assert(sai.rx_fifo[channel][1] == channel * 2 + 2);
    }
    for (unsigned channel = 0; channel < L6_SAI_CHANNELS; channel++) {
        assert(l6_sai_read(&sai, L6_SAI_RDR0, 4) == channel * 2 + 1);
    }
    for (unsigned channel = 0; channel < L6_SAI_CHANNELS; channel++) {
        assert(l6_sai_read(&sai, L6_SAI_RDR0, 4) == channel * 2 + 2);
    }
    assert(!(l6_sai_read(&sai, L6_SAI_RCSR, 4) & L6_SAI_FRF));

    /* FCOMB=write distributes a single input word stream over enabled FIFOs. */
    l6_sai_fifo_reset(&sai, false);
    sai.regs[L6_SAI_RCR4 / 4] = L6_SAI_RX_FCOMB_WRITE |
                                (3u << L6_SAI_FRSZ_SHIFT);
    uint32_t combined_rx_frame[4] = { 11, 12, 13, 14 };
    l6_sai_rx_frame(&sai, combined_rx_frame, 4);
    for (unsigned channel = 0; channel < L6_SAI_CHANNELS; channel++) {
        assert(sai.rx_count[channel] == 1);
        assert(sai.rx_fifo[channel][0] == channel + 11);
    }

    /* FCOMB=read/write combines both incoming writes and software reads. */
    l6_sai_fifo_reset(&sai, false);
    sai.regs[L6_SAI_RCR4 / 4] = L6_SAI_RX_FCOMB_READ_WRITE |
                                (3u << L6_SAI_FRSZ_SHIFT);
    uint32_t combined_rx_read_write[4] = { 21, 22, 23, 24 };
    l6_sai_rx_frame(&sai, combined_rx_read_write, 4);
    for (unsigned channel = 0; channel < L6_SAI_CHANNELS; channel++) {
        assert(l6_sai_read(&sai, L6_SAI_RDR0, 4) == channel + 21);
    }

    puts("RT SAI register/multi-FIFO checks passed");
    return 0;
}
