#!/usr/bin/env bash
# Pull the Configurator's host->device traffic out of a USBPcap capture.
# Usage: ./analyze_d700_usb.sh usbcap/d700_hub3.pcap
set -u
TSHARK="/c/Program Files/Wireshark/tshark.exe"
PCAP="${1:-usbcap/d700_hub3.pcap}"

[ -f "$PCAP" ] || { echo "no such capture: $PCAP"; exit 1; }
echo "=== $PCAP  ($(stat -c%s "$PCAP" 2>/dev/null || echo ?) bytes) ==="

echo
echo "--- packet totals by endpoint / transfer type ---"
"$TSHARK" -r "$PCAP" -T fields \
  -e usb.device_address -e usb.endpoint_address -e usb.transfer_type -e usb.urb_type \
  2>/dev/null | sort | uniq -c | sort -rn | head -25

echo
echo "--- HOST -> DEVICE payloads (the Configurator talking) ---"
echo "    submit URBs carrying data, any endpoint"
"$TSHARK" -r "$PCAP" -Y 'usb.urb_type == 0x53 && usb.data_len > 0' \
  -T fields -e frame.number -e usb.endpoint_address -e usb.transfer_type \
  -e usb.data_len -e usb.capdata 2>/dev/null | head -60

echo
echo "--- SETUP / control transfers (SET_REPORT lives here) ---"
"$TSHARK" -r "$PCAP" -Y 'usb.transfer_type == 0x02' \
  -T fields -e frame.number -e usb.bmRequestType -e usb.setup.bRequest \
  -e usb.setup.wValue -e usb.setup.wIndex -e usb.data_len -e usb.capdata \
  2>/dev/null | head -40

echo
echo "--- distinct host->device payloads, most frequent first ---"
"$TSHARK" -r "$PCAP" -Y 'usb.urb_type == 0x53 && usb.data_len > 0' \
  -T fields -e usb.capdata 2>/dev/null | sort | uniq -c | sort -rn | head -30
