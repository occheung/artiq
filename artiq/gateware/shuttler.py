from migen.build.generic_platform import *


fmc_adapter_io = [
    ("user_led", 0, Pins("fmc0:HA23_N"), IOStandard("LVTTL")),
    ("user_led", 1, Pins("fmc0:HA23_P"), IOStandard("LVTTL")),
    ("user_led", 2, Pins("fmc0:LA32_P"), IOStandard("LVTTL")),
    ("user_led", 3, Pins("fmc0:HB18_N"), IOStandard("LVTTL")),

    # ("dac_spi", 0,
    #     Subsignal("clk", Pins("fmc0:HB16_N")),
    #     Subsignal("mosi", Pins("fmc0:HB06_CC_N")),
    #     # Real DAC chip select will be its own signal
    #     Subsignal("cs_n", Pins("fmc0:LA31_N")),
    #     IOStandard("LVCMOS25")),

    # ("dac_cs_a", 0, Pins("fmc0:LA31_P"), IOStandard("LVCMOS25")),
    # ("dac_cs_a", 1, Pins("fmc0:HB19_P"), IOStandard("LVCMOS25")),
    # ("dac_cs_a", 2, Pins("fmc0:LA30_P"), IOStandard("LVCMOS25")),

    # ("dac_rst", 0, Pins("fmc0:HB16_P"), IOStandard("LVCMOS25")),
]