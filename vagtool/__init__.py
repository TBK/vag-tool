"""vagtool - KWP2000-over-TP2.0 diagnostics for VAG PQ35 cars on SocketCAN.

Written for an Audi A3 8P 2.0 TDI PD (BMN, Siemens PPD1.x) and a raw USB-CAN
adapter such as the 8devices Korlan. No third-party dependencies.
"""

__version__ = "0.1.0"

from .blocks import decode_group, format_group
from .canbus import SocketCanBus, open_bus
from .kwp2000 import Kwp2000Client, NegativeResponse
from .tp20 import Tp20Channel, Tp20Error

__all__ = ["SocketCanBus", "open_bus", "Tp20Channel", "Tp20Error", "Kwp2000Client",
           "NegativeResponse", "decode_group", "format_group", "__version__"]
