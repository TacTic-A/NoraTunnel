# 🚀 NoraTunnel (v3.0.0)

<div align="center">

**Enterprise High-Performance Reverse Tunnel & Traffic Relay System**  
*Engineered for maximum throughput, low latency, Noise protocol obfuscation, and self-healing resilience.*

[![Release](https://img.shields.io/badge/Release-v3.0.0-blue.svg)](https://github.com/TacTic-A/NoraTunnel/releases)
[![License](https://img.shields.io/badge/License-MIT-green.svg)](LICENSE)
[![Platform](https://img.shields.io/badge/Platform-Linux%20(Ubuntu%20%7C%20Debian%20%7C%20RHEL%20%7C%20Arch)-orange.svg)]()
[![Core](https://img.shields.io/badge/Engine-Rust%20High--Speed-red.svg)]()
[![Security](https://img.shields.io/badge/Obfuscation-Noise%20Protocol%20(X25519)-purple.svg)]()

---

</div>

## Table of Contents
- [Overview](#-overview)
- [Key Features](#-key-features)
- [Quick Installation](#-quick-installation)
- [Step-by-Step Setup](#-step-by-step-setup)
- [Built-In Diagnostic & Reliability Tools](#-built-in-diagnostic--reliability-tools)
- [CLI Commands & Shortcuts](#-cli-commands--shortcuts)
- [Kernel & System Tuning (BBR)](#-kernel--system-tuning-bbr)
- [Architecture Details](#-architecture-details)
- [License](#-license)

---

## 🌟 Overview

**NoraTunnel** is an advanced, lightweight reverse tunneling and network traffic relay orchestrator. Designed for stability and exceptional throughput, it bridges traffic between distributed server endpoints with minimal CPU and memory overhead (< 15MB RAM).

Powered by a native, precompiled Rust engine and paired with an interactive bash management CLI, NoraTunnel automates daemon creation, firewall adaptation, port forwarding, self-healing watchdogs, and kernel-level network optimization.

---

## ⚡ Key Features

- **Blazing Fast Performance:** Powered by a lightweight Rust core with optimized buffer pooling and zero unnecessary allocations.
- **Noise Protocol Obfuscation:** Native cryptographic transport layer using **X25519** and **ChaCha20-Poly1305** to obfuscate packets into random noise, completely defeating Deep Packet Inspection (DPI) and protocol fingerprinting.
- **Dual-Stack Support:** Full native support for both **IPv4** and **IPv6** endpoints.
- **Multi-Port Forwarding:** Bridge unlimited TCP and UDP services simultaneously with comma-separated port definitions.
- **Automated Firewall Integration:** Automatically opens tunnel and service ports in **UFW** and **Firewalld** upon configuration.
- **Self-Healing Watchdog:** Lightweight background daemon continuously monitors tunnel instances and revives stalled connections within 45 seconds.
- **Live Latency & Reachability Tester:** Built-in TCP handshake tool measures packet loss and millisecond-precision round-trip time (RTT).
- **One-Click Backup & Restore:** Export all tunnel profiles, keys, and daemons into a compressed archive for instant server migration.
- **Isolated Service Supervision:** Individual `systemd` daemon management with isolated, non-interfering scheduled restarts.
- **Instant CLI Shortcut:** Auto-installs `nora` and `noratunnel` shortcuts for instant terminal access from any directory.
- **Automated Kernel Tuning:** One-click integration of Google's TCP BBR congestion control, FQ-Codel queue discipline, and socket limits scaling (`ulimit -n 1048576`).

---

## 🚀 Quick Installation

Run the following command with root privileges on any Linux server (Ubuntu, Debian, CentOS, AlmaLinux, Rocky, Arch):

```bash
bash <(curl -fsSL https://raw.githubusercontent.com/TacTic-A/NoraTunnel/main/NoraTunnel.sh)
```

Once installed, launch the management console anytime by simply typing:

```bash
nora
```

---

## 🛠️ Step-by-Step Setup

NoraTunnel establishes an encrypted reverse tunnel between a **Server Node** (Listener) and a **Client Node** (Forwarder).

### 1. Configure the Server Node (Listener)
1. Run `nora` in your terminal.
2. Select option **`1. Configure a new NoraTunnel`**.
3. Choose **`1) Configure as IRAN Server`** (Server / Listener Mode).
4. Specify your preferences:
   - **IPv6 Support:** Choose whether to listen on `[::]` or `0.0.0.0`.
   - **Tunnel Port:** Main bridge communication port (e.g., `2080`).
   - **Obfuscation Layer:** Select **Option 1 (Noise Protocol)** for encrypted, DPI-proof tunneling.
   - **Service Ports:** Comma-separated list of ports you wish to bridge (e.g., `80,443,2053,8080`).
   - **Security Token:** Press Enter to auto-generate a secure random 256-bit token.
5. Upon completion, a summary card will display the connection parameters and the generated **Noise Public Key**.

### 2. Configure the Client Node (Forwarder)
1. Run `nora` on your client server.
2. Select option **`1. Configure a new NoraTunnel`**.
3. Choose **`2) Configure as KHAREJ Server`** (Client / Forwarder Mode).
4. Enter the matching parameters from Step 1:
   - **Server IP:** The public IP address of your Server node.
   - **Tunnel Port:** The bridge port configured on the Server node (e.g., `2080`).
   - **Obfuscation Layer:** Select **Option 1 (Noise Protocol)**.
   - **Noise Public Key:** Paste the public key displayed on your Server node.
   - **Security Token:** The exact token generated on the Server node.
   - **Bridged Ports:** The matching service ports (e.g., `80,443,2053,8080`).
5. NoraTunnel will automatically register, configure firewall rules, and start the `systemd` daemon.

---

## 🔍 Built-In Diagnostic & Reliability Tools

### 1. Connectivity & Latency Ping Tester (Option 4)
Verify TCP reachability and measure round-trip latency to any remote IP and port:
- Checks if ports are open or filtered before launching client services.
- Sends consecutive TCP handshakes to calculate packet loss and jitter.

### 2. Smart Watchdog Auto-Recovery (Option 5)
A background watchdog daemon that scans NoraTunnel services every 45 seconds:
- Automatically detects dead or stalled tunnel processes.
- Gracefully revives instances with zero downtime on operational tunnels.

### 3. Backup & Restore (Option 6)
- **Backup:** Archives `/root/noratunnel-core/` and systemd unit files into `/root/noratunnel_backup_*.tar.gz`.
- **Restore:** Extracts the archive, re-registers daemons with systemd, and brings all tunnels back online instantly.

---

## 🎛️ CLI Commands & Shortcuts

| Command | Description |
| :--- | :--- |
| `nora` | Launch the NoraTunnel interactive management dashboard |
| `noratunnel` | Alternative full command to launch the dashboard |
| `systemctl status noratunnel-iran<PORT>` | Check status of server daemon |
| `systemctl status noratunnel-kharej<PORT>` | Check status of client daemon |
| `journalctl -u noratunnel-watchdog -f` | Inspect watchdog monitoring logs |
| `journalctl -u noratunnel-iran<PORT> -f` | Stream live daemon logs |

---

## ⚙️ Kernel & System Tuning (BBR)

Selecting option **`7. Optimize Network & System Limits`** applies enterprise-grade Linux kernel optimizations:

- **TCP Congestion Control:** Activates Google's **BBR** (Bottleneck Bandwidth and RTT) algorithm.
- **Buffer Scaling:** Expands TCP read/write memory buffers up to 32MB for maximum bandwidth saturation.
- **Connection Backlog:** Increases `somaxconn` to `65536` and `netdev_max_backlog` to `32768`.
- **Descriptor Limits:** Raises process open file descriptor limits to `1,048,576` in both system profiles and systemd unit files.

---

## 📄 License

This project is licensed under the [MIT License](LICENSE) - Copyright (c) 2026 TacTic-A.
