from migen import *
from artiq.gateware.rtio import rtlink


class ShuttlerTrigger(Module):
    def __init__(self):
        self.rtlink = rtlink.Interface(
            rtlink.OInterface(data_width=16)
        )

        self.triggers = Signal(16)

        self.sync.rio_phy += If(self.rtlink.o.stb,
            self.triggers.eq(self.rtlink.o.data),
        )


class ShuttlerLaneRegister(Module):
    def __init__(self, dacs, triggers):
        # Register access interface:
        # Addr 0: Config
        # Addr 1: Frame
        self.rtlink = rtlink.Interface(
            rtlink.OInterface(data_width=8, address_width=1),
            # TODO: Do we seriously need a readback?
            rtlink.IInterface(data_width=8),
        )

        self.config = Record([
            ("reset", 1),       # TODO: Remove, use RTIO reset
            ("clk2x", 1),       # TODO: Remove
            ("enable", 1),
            ("trigger", 1),
            ("aux_miso", 1),    # TODO: Remove
            ("aux_dac", 3),     # TODO: Ignore
        ])
        self.frame = Signal(max=32)
        reg_map = Array([self.config.raw_bits(), self.frame])

        self.sync.rio_phy += If(self.rtlink.o.stb,
            reg_map[self.rtlink.o.address].eq(self.rtlink.o.data)
        )

        # TODO: Remove
        self.sync.rio_phy += [
            self.rtlink.i.stb.eq(self.rtlink.o.stb),
            self.rtlink.i.data.eq(0xDE),
        ]

        self.probes = []
        self.overrides = []

        for dac, trigger in zip(dacs, triggers):
            self.comb += [
                dac.parser.frame.eq(self.frame),
                dac.out.trigger.eq(self.config.enable &
                                    (trigger | self.config.trigger)),
                dac.out.arm.eq(self.config.enable),
                dac.parser.arm.eq(self.config.enable),
                dac.parser.start.eq(self.config.enable),
            ]


class ShuttlerMemory(Module):
    def __init__(self, dac):
        # Register access interface:
        # - 0: Memory address write. Address automatically increments after memory write
        # - 1: Memory write
        self.rtlink = rtlink.Interface(
            rtlink.OInterface(data_width=16, address_width=1),
        )

        mem_adr = Signal(16)
        self.specials.wport = dac.parser.mem.get_port(write_capable=True, clock_domain="rio_phy")

        self.sync.rio_phy += [
            # Disable memory write by default
            self.wport.we.eq(0),

            If(self.rtlink.o.stb,
                If(self.rtlink.o.address == 0,
                    mem_adr.eq(self.rtlink.o.data),
                ).Elif(self.rtlink.o.address == 1,
                    self.wport.adr.eq(mem_adr),
                    self.wport.dat_w.eq(self.rtlink.o.data),
                    # Only assert "write enable" in this block
                    self.wport.we.eq(1),
                    mem_adr.eq(mem_adr + 1),
                )
            ),
        ]


class ShuttlerMonitor(Module):
    def __init__(self, dacs):
        # Logger interface:
        # Select channel by address
        # Create input event by pulsing OInterface stb
        self.rtlink = rtlink.Interface(
            rtlink.OInterface(data_width=0, address_width=4),
            rtlink.IInterface(data_width=16),
        )

        data_out = Array(dac.out.data for dac in dacs)

        self.comb += [
            self.rtlink.i.stb.eq(self.rtlink.o.stb),
            # self.rtlink.i.data.eq(data_out[self.rtlink.o.address]),
            self.rtlink.i.data.eq(dacs[0].out.data),
        ]
