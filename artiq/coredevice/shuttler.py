import numpy

from artiq.language.core import *
from artiq.language.types import *
from artiq.coredevice.rtio import (rtio_output, rtio_input_timestamp,
                                   rtio_input_data)
from artiq.coredevice.exceptions import RTIOOverflow


class ShuttlerRegister:
    kernel_invariants = {"core", "channel", "target_o"}

    def __init__(self, dmgr, channel, core_device="core"):
        self.core = dmgr.get(core_device)
        self.channel = channel
        self.target_o = channel << 8
    
    @kernel
    def set_config(self, config):
        rtio_output(self.target_o, config)
    
    @kernel
    def set_frame(self, frame):
        rtio_output(self.target_o | 1, frame)

    @kernel
    def sample_get(self):
        """Returns the value of a sample previously obtained with
        :meth:`sample_input`.

        Multiple samples may be queued (using multiple calls to
        :meth:`sample_input`) into the RTIO FIFOs and subsequently read out using
        multiple calls to this function.

        This function does not interact with the time cursor."""
        return rtio_input_data(self.channel)


class ShuttlerTrigger:
    kernel_invariants = {"core", "channel", "target_o"}

    def __init__(self, dmgr, channel, core_device="core"):
        self.core = dmgr.get(core_device)
        self.channel = channel
        self.target_o = channel << 8
    
    @kernel
    def trigger(self, ch_bits):
        rtio_output(self.target_o, ch_bits)


class ShuttlerMemory:
    kernel_invariants = {"core", "channel", "target_o"}

    def __init__(self, dmgr, channel, core_device="core"):
        self.core = dmgr.get(core_device)
        self.channel = channel
        self.target_o = channel << 8
    
    @kernel
    def write_mem(self, addr, data):
        """Write to memory by:
            1. Write memory address
            2. Write data, 16-bits word each time

            This method advances timeline cursor after each RTIO output
        """
        rtio_output(self.target_o, addr)
        for word in data:
            delay_mu(8)
            rtio_output(self.target_o | 1, word)


class ShuttlerMonitor:
    kernel_invariants = {"core", "channel", "target_o"}

    def __init__(self, dmgr, channel, core_device="core"):
        self.core = dmgr.get(core_device)
        self.channel = channel
        self.target_o = channel << 8

    @kernel
    def sample(self, dac):
        # The monitor has no output bits, use 0 as a dummy
        rtio_output(self.target_o | dac, 0)

    @kernel
    def sample_get(self):
        """Returns the value of a sample previously obtained with
        :meth:`sample`.

        Multiple samples may be queued (using multiple calls to
        :meth:`sample`) into the RTIO FIFOs and subsequently read out using
        multiple calls to this function.

        This function does not interact with the time cursor."""
        return rtio_input_data(self.channel)
