# Copyright 2013-2017 Robert Jordens <jordens@gmail.com>
#
# This file is part of pdq.
#
# pdq is free software: you can redistribute it and/or modify
# it under the terms of the GNU General Public License as published by
# the Free Software Foundation, either version 3 of the License, or
# (at your option) any later version.
#
# pdq is distributed in the hope that it will be useful,
# but WITHOUT ANY WARRANTY; without even the implied warranty of
# MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
# GNU General Public License for more details.
#
# You should have received a copy of the GNU General Public License
# along with pdq.  If not, see <http://www.gnu.org/licenses/>.

from migen import *
from migen.genlib.record import Record
from migen.genlib.resetsync import AsyncResetSynchronizer

from .dac import Dac


class PdqBase(Module):
    """PDQ Base configuration.

    Used both in functional simulation and final gateware.

    Holds the three :mod:`gateware.dac.Dac` and the communication handler
    :mod:`gateware.comm.Comm`.

    Args:
        ctrl_pads (Record): Control pads for :mod:`gateware.comm.Comm`.
        mem_depth (list[int]): Memory depths for the DAC channels.

    Attributes:
        dacs (list): List of :mod:`gateware.dac.Dac`.
        comm (Module): :mod:`gateware.comm.Comm`.
    """
    def __init__(self, mem_depths=(1 << 13, 1 << 13, 1 << 12)):
        self.dacs = []
        for i in range(16):
            dac = Dac(mem_depth=(1 << 12))
            setattr(self.submodules, "dac{}".format(i), dac)
            self.dacs.append(dac)
        # TODO: Restructure, and replace with RTIO interface
        # self.submodules.comm = Comm(ctrl_pads, self.dacs)


@SplitMemory()
@FullMemoryWE()
class Pdq(PdqBase):
    """PDQ Top module.

    Wires up USB FIFO reader :mod:`gateware.ft245r.Ft345r_rx`, clock and reset
    generator :mod:`CRG`, and the DAC output signals.
    Delegates the wiring of the remaining modules to :mod:`PdqBase`.

    ``pads.g2_out`` is assigned the DCM locked signal.

    Args:
        platform (Platform): PDQ platform.
    """
    def __init__(self, platform, **kwargs):
        self.platform = platform
        PdqBase.__init__(self, **kwargs)
        # self.submodules.crg = CRG(platform)
        # comm_pads = platform.request("comm")
        # self.submodules.reader = Ft245r_rx(comm_pads)
        # self.comb += [
        #         self.reader.source.connect(self.comm.ftdi_bus),
        #         self.crg.rst.eq(self.comm.rg.reset),
        #         ctrl_pads.g2_out.eq(self.crg.dcm_locked),
        #         self.crg.dcm_sel.eq(self.comm.proto.config.clk2x),
        #         ctrl_pads.reset.eq(ResetSignal()),
        # ]

        # TODO: Add DAC PHY
        # sys_p, sys_n = ClockSignal("sys"), ClockSignal("sys_n")
        # for i, dac in enumerate(self.dacs):
        #     pads = platform.request("dac", i)
        #     # inverted clocks ensure setup and hold times of data
        #     ce = Signal()
        #     d = Signal.like(dac.out.data)
        #     self.comb += [
        #             ce.eq(~dac.out.silence),
        #             d.eq(~dac.out.data),  # pcb inversion
        #     ]

        #     self.specials += Instance("ODDR2",
        #             i_C0=sys_p, i_C1=sys_n, i_CE=ce,
        #             i_D0=0, i_D1=1, i_R=0, i_S=0, o_Q=pads.clk_p)
        #     self.specials += Instance("ODDR2",
        #             i_C0=sys_p, i_C1=sys_n, i_CE=ce,
        #             i_D0=1, i_D1=0, i_R=0, i_S=0, o_Q=pads.clk_n)
        #     dclk = Signal()
        #     self.specials += Instance("ODDR2",
        #             i_C0=sys_p, i_C1=sys_n, i_CE=ce,
        #             i_D0=0, i_D1=1, i_R=0, i_S=0, o_Q=dclk)
        #     self.specials += Instance("OBUFDS",
        #             i_I=dclk, o_O=pads.data_clk_p, o_OB=pads.data_clk_n)
        #     for i in range(16):
        #         self.specials += Instance("OBUFDS",
        #                 i_I=d[i], o_O=pads.data_p[i], o_OB=pads.data_n[i])
