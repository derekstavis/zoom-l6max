/* Count L6 MAIN functions identified in eDMA IRQ 15 samples. */
#include <inttypes.h>
#include <stdint.h>
#include <stdio.h>

#include "qemu-plugin.h"

QEMU_PLUGIN_EXPORT int qemu_plugin_version = QEMU_PLUGIN_VERSION;

#define MAX_VCPUS 64
#define PROFILED_FUNCTIONS 4

typedef struct {
    const char *name;
    uint64_t start;
    uint64_t end;
} ProfiledFunction;

static const ProfiledFunction functions[PROFILED_FUNCTIONS] = {
    { "l6_sai1_rx_dma_callback", 0x800335f8ULL, 0x80033750ULL },
    { "vPortValidateInterruptPriority", 0x80074058ULL, 0x800740b6ULL },
    { "l6_edma_irq15_handler", 0x8001e1a8ULL, 0x8001e1fcULL },
    { "l6_edma_tcd_completion_dispatch", 0x8001e8b8ULL, 0x8001e980ULL },
};

static uint64_t instruction_counts[MAX_VCPUS][PROFILED_FUNCTIONS];

static void count_instruction(unsigned int vcpu_index, void *userdata)
{
    uintptr_t function_index = (uintptr_t)userdata;
    if (vcpu_index < MAX_VCPUS && function_index < PROFILED_FUNCTIONS) {
        __atomic_fetch_add(&instruction_counts[vcpu_index][function_index], 1,
                           __ATOMIC_RELAXED);
    }
}

static void translate_block(struct qemu_plugin_tb *tb, void *userdata)
{
    size_t count = qemu_plugin_tb_n_insns(tb);

    for (size_t i = 0; i < count; i++) {
        struct qemu_plugin_insn *insn = qemu_plugin_tb_get_insn(tb, i);
        uint64_t pc = qemu_plugin_insn_vaddr(insn);

        for (uintptr_t function_index = 0;
             function_index < PROFILED_FUNCTIONS; function_index++) {
            if (pc >= functions[function_index].start &&
                pc < functions[function_index].end) {
                qemu_plugin_register_vcpu_insn_exec_cb(
                    insn, count_instruction, QEMU_PLUGIN_CB_NO_REGS,
                    (void *)function_index);
                break;
            }
        }
    }
}

static void report_counts(void *userdata)
{
    for (unsigned int cpu = 0; cpu < MAX_VCPUS; cpu++) {
        for (unsigned int function_index = 0;
             function_index < PROFILED_FUNCTIONS; function_index++) {
            uint64_t count = __atomic_load_n(
                &instruction_counts[cpu][function_index], __ATOMIC_RELAXED);
            if (count) {
                fprintf(stderr,
                        "l6-irq-profile: vcpu=%u %s-instructions=%" PRIu64 "\n",
                        cpu, functions[function_index].name, count);
            }
        }
    }
}

QEMU_PLUGIN_EXPORT int qemu_plugin_install(qemu_plugin_id_t id,
                                           const qemu_info_t *info,
                                           int argc, char **argv)
{
    if (!info->system_emulation || info->system.max_vcpus > MAX_VCPUS) {
        fprintf(stderr, "l6-sai1-rx-profile: unsupported QEMU configuration\n");
        return -1;
    }

    qemu_plugin_register_vcpu_tb_trans_cb(id, translate_block, NULL);
    qemu_plugin_register_atexit_cb(id, report_counts, NULL);
    return 0;
}
