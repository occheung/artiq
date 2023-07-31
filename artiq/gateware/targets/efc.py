#!/usr/bin/env python3

import argparse

from migen import *
from migen.build.generic_platform import *

from misoc.interconnect.csr import *
from misoc.cores import gpio
from misoc.targets.efc import BaseSoC
from misoc.integration.builder import builder_args, builder_argdict

from artiq.gateware.amp import AMPSoC
from artiq.gateware import rtio, shuttler
from artiq.gateware.rtio.phy import ttl_simple, shuttler as shuttler_rtio
from artiq.gateware.rtio.xilinx_clocking import fix_serdes_timing_path
from artiq.gateware.drtio.transceiver import eem_serdes
from artiq.gateware.drtio.rx_synchronizer import XilinxRXSynchronizer
from artiq.gateware.drtio import *
from artiq.build_soc import *

from artiq.gateware.rtio.phy.pdq.pdq import Pdq


class SatelliteBase(BaseSoC, AMPSoC):
    mem_map = {
        "rtio":          0x20000000,
        "drtioaux":      0x50000000,
        "mailbox":       0x70000000
    }
    mem_map.update(BaseSoC.mem_map)

    def __init__(self, rtio_clk_freq=125e6, enable_sata=False, *, gateware_identifier_str=None, hw_rev="v2.0", **kwargs):
        if hw_rev in ("v1.0", "v1.1"):
            cpu_bus_width = 32
        else:
            cpu_bus_width = 64
        BaseSoC.__init__(self,
                 cpu_type="vexriscv",
                 cpu_bus_width=cpu_bus_width,
                 sdram_controller_type="minicon",
                 l2_size=128*1024,
                 clk_freq=rtio_clk_freq,
                 **kwargs)
        AMPSoC.__init__(self)
        add_identifier(self, gateware_identifier_str=gateware_identifier_str)

        platform = self.platform
        platform.add_extension(shuttler.fmc_adapter_io)

        drtio_eem_io = [
            ("drtio_data_out", 0,
                Subsignal("p", Pins("eem0:d0_cc_p eem0:d1_p eem0:d2_p eem0:d3_p")),
                Subsignal("n", Pins("eem0:d0_cc_n eem0:d1_n eem0:d2_n eem0:d3_n")),
                IOStandard("LVDS_25"),
            ),
            ("drtio_data_in", 0,
                Subsignal("p", Pins("eem0:d4_p eem0:d5_p eem0:d6_p eem0:d7_p")),
                Subsignal("n", Pins("eem0:d4_n eem0:d5_n eem0:d6_n eem0:d7_n")),
                IOStandard("LVDS_25"), Misc("DIFF_TERM=TRUE"),
            ),
        ]

        platform.add_extension(drtio_eem_io)
        data_pads = [
            (platform.request("drtio_data_in"), platform.request("drtio_data_out"))
        ]

        # Disable SERVMOD, hardwire it to ground to enable EEM
        servmod = self.platform.request("servmod")
        self.comb += servmod.eq(0)

        self.submodules.eem_transceiver = eem_serdes.EEMSerdes(self.platform, data_pads)
        self.csr_devices.append("eem_transceiver")
        self.config["HAS_DRTIO_EEM"] = None

        self.submodules.serdes_crg = eem_serdes.SerdesCRG(
            self.platform, self.crg.clk125_div2, 62.5e6, False)
        self.csr_devices.append("serdes_crg")

        platform.add_false_path_constraint(self.crg.cd_sys.clk, self.serdes_crg.cd_eem_sys.clk)

        self.submodules.rtio_tsc = rtio.TSC(glbl_fine_ts_width=3)

        drtioaux_csr_group = []
        drtioaux_memory_group = []
        drtiorep_csr_group = []
        self.drtio_cri = []
        for i in range(len(self.eem_transceiver.channels)):
            coreaux_name = "drtioaux" + str(i)
            memory_name = "drtioaux" + str(i) + "_mem"
            drtioaux_csr_group.append(coreaux_name)
            drtioaux_memory_group.append(memory_name)

            cdr = ClockDomainsRenamer({"rtio_rx": "rtio_rx" + str(i)})

            if i == 0:
                self.submodules.rx_synchronizer = cdr(XilinxRXSynchronizer())
                core = cdr(DRTIOSatellite(
                    self.rtio_tsc, self.eem_transceiver.channels[i],
                    self.rx_synchronizer))
                self.submodules.drtiosat = core
                self.csr_devices.append("drtiosat")
            else:
                corerep_name = "drtiorep" + str(i-1)
                drtiorep_csr_group.append(corerep_name)

                core = cdr(DRTIORepeater(
                    self.rtio_tsc, self.eem_transceiver.channels[i]))
                setattr(self.submodules, corerep_name, core)
                self.drtio_cri.append(core.cri)
                self.csr_devices.append(corerep_name)

            coreaux = cdr(DRTIOAuxController(core.link_layer, self.cpu_dw))
            setattr(self.submodules, coreaux_name, coreaux)
            self.csr_devices.append(coreaux_name)

            memory_address = self.mem_map["drtioaux"] + 0x800*i
            self.add_wb_slave(memory_address, 0x800,
                              coreaux.bus)
            self.add_memory_region(memory_name, memory_address | self.shadow_base, 0x800)
        self.config["HAS_DRTIO"] = None
        self.add_csr_group("drtioaux", drtioaux_csr_group)
        self.add_memory_group("drtioaux_mem", drtioaux_memory_group)
        self.add_csr_group("drtiorep", drtiorep_csr_group)

        i2c = self.platform.request("fpga_i2c")
        self.submodules.i2c = gpio.GPIOTristate([i2c.scl, i2c.sda])
        self.csr_devices.append("i2c")
        self.config["I2C_BUS_COUNT"] = 1

        # Enable I2C
        i2c_reset = self.platform.request("i2c_mux_rst_n")
        self.comb += i2c_reset.eq(1)

        rtio_clk_period = 1e9/rtio_clk_freq
        self.config["RTIO_FREQUENCY"] = str(rtio_clk_freq/1e6)

        fix_serdes_timing_path(platform)

    def add_rtio(self, rtio_channels, sed_lanes=8):
        # Only add MonInj core if there is anything to monitor
        if any([len(c.probes) for c in rtio_channels]):
            self.submodules.rtio_moninj = rtio.MonInj(rtio_channels)
            self.csr_devices.append("rtio_moninj")

        # satellite (master-controlled) RTIO
        self.submodules.local_io = SyncRTIO(self.rtio_tsc, rtio_channels, lane_count=sed_lanes)
        self.comb += self.drtiosat.async_errors.eq(self.local_io.async_errors)

        # subkernel RTIO
        self.submodules.rtio = rtio.KernelInitiator(self.rtio_tsc)
        self.register_kernel_cpu_csrdevice("rtio")

        self.submodules.rtio_dma = rtio.DMA(self.get_native_sdram_if(), self.cpu_dw)
        self.csr_devices.append("rtio_dma")
        self.submodules.cri_con = rtio.CRIInterconnectShared(
            [self.drtiosat.cri, self.rtio_dma.cri],
            [self.local_io.cri] + self.drtio_cri,
            enable_routing=True)
        self.csr_devices.append("cri_con")
        self.submodules.routing_table = rtio.RoutingTableAccess(self.cri_con)
        self.csr_devices.append("routing_table")

        self.submodules.rtio_analyzer = rtio.Analyzer(self.rtio_tsc, self.local_io.cri,
                                                self.get_native_sdram_if(), cpu_dw=self.cpu_dw)
        self.csr_devices.append("rtio_analyzer")


class Satellite(SatelliteBase):
    def __init__(self, hw_rev=None, **kwargs):
        if hw_rev is None:
            hw_rev = "v2.0"
        SatelliteBase.__init__(self, hw_rev=hw_rev, **kwargs)
        self.config["DRTIO_ROLE"] = "satellite"

        self.rtio_channels = []
        for i in range(2):
            print("USER LED at RTIO channel 0x{:06x}".format(len(self.rtio_channels)))
            phy = ttl_simple.Output(self.platform.request("user_led", i))
            self.submodules += phy
            self.rtio_channels.append(rtio.Channel.from_phy(phy))
        
        for i in range(2, 4):
            led = self.platform.request("user_led", i)
            if i % 2:
                self.comb += led.eq(1)
            else:
                self.comb += led.eq(0)
        
        # Add shuttler
        self.submodules.shuttler = ClockDomainsRenamer("rio_phy")(Pdq(self.platform))

        print("SHUTTLER TRIGGER at RTIO channel 0x{:06x}".format(len(self.rtio_channels)))
        trig_phy = shuttler_rtio.ShuttlerTrigger()
        self.submodules += trig_phy
        self.rtio_channels.append(rtio.Channel.from_phy(trig_phy))

        print("SHUTTLER REGISTER at RTIO channel 0x{:06x}".format(len(self.rtio_channels)))
        register_phy = shuttler_rtio.ShuttlerLaneRegister(self.shuttler.dacs, trig_phy.triggers)
        self.submodules += register_phy
        self.rtio_channels.append(rtio.Channel.from_phy(register_phy))

        print("SHUTTLER MONITOR at RTIO channel 0x{:06x}".format(len(self.rtio_channels)))
        mon_phy = shuttler_rtio.ShuttlerMonitor(self.shuttler.dacs)
        self.submodules += mon_phy
        self.rtio_channels.append(rtio.Channel.from_phy(mon_phy))

        for dac in self.shuttler.dacs:
            print("SHUTTLER MEMORY at RTIO channel 0x{:06x}".format(len(self.rtio_channels)))
            shuttler_mem = shuttler_rtio.ShuttlerMemory(dac)
            self.submodules += shuttler_mem
            self.rtio_channels.append(rtio.Channel.from_phy(shuttler_mem))

        self.config["HAS_RTIO_LOG"] = None
        self.config["RTIO_LOG_CHANNEL"] = len(self.rtio_channels)
        self.rtio_channels.append(rtio.LogChannel())

        self.add_rtio(self.rtio_channels)


VARIANTS = {cls.__name__.lower(): cls for cls in [Satellite]}


def main():
    parser = argparse.ArgumentParser(
        description="ARTIQ device binary builder for EEM FMC Carrier systems")
    builder_args(parser)
    parser.set_defaults(output_dir="artiq_efc")
    parser.add_argument("-V", "--variant", default="satellite",
                        help="variant: {} (default: %(default)s)".format(
                            "/".join(sorted(VARIANTS.keys()))))
    parser.add_argument("--gateware-identifier-str", default=None,
                        help="Override ROM identifier")
    args = parser.parse_args()

    argdict = dict()
    argdict["gateware_identifier_str"] = args.gateware_identifier_str

    variant = args.variant.lower()
    try:
        cls = VARIANTS[variant]
    except KeyError:
        raise SystemExit("Invalid variant (-V/--variant)")

    soc = cls(**argdict)
    build_artiq_soc(soc, builder_argdict(args))


if __name__ == "__main__":
    main()
