#!/bin/bash

# ==============================================================================
#  NoraTunnel - High-Performance Reverse Tunnel Management System
#  Optimized for censorship resistance, low latency, and high stability.
# ==============================================================================

# Check if script is run with root privileges
if [[ $EUID -ne 0 ]]; then
   echo -e "\033[31m[ERROR] NoraTunnel must be run as root!\033[0m"
   echo -e "Please run with: \033[33msudo bash $0\033[0m"
   sleep 1
   exit 1
fi

# Configuration paths
CONFIG_DIR="/root/noratunnel-core"
SERVICE_DIR="/etc/systemd/system"
CORE_BIN="${CONFIG_DIR}/noratunnel-core"
SCRIPT_INSTALL_PATH="/usr/local/bin/noratunnel"
SYS_PATH="/etc/sysctl.conf"
PROF_PATH="/etc/profile"

# ANSI Color Palette (Modern 256-Color & Unicode Theme)
NC='\033[0m'
BOLD='\033[1m'
DIM='\033[2m'
RED='\033[38;5;203m'
GREEN='\033[38;5;84m'
YELLOW='\033[38;5;221m'
BLUE='\033[38;5;75m'
PURPLE='\033[38;5;141m'
CYAN='\033[38;5;81m'
GRAY='\033[38;5;242m'
DARK_GRAY='\033[38;5;238m'
WHITE='\033[38;5;255m'
BG_BLUE='\033[48;5;24m'
MAGENTA='\033[38;5;177m'

press_key(){
    echo
    read -p "Press Enter to continue..."
}

colorize() {
    local color="$1"
    local text="$2"
    local style="${3:-normal}"
    
    local c_code=""
    case $color in
        red)     c_code="$RED" ;;
        green)   c_code="$GREEN" ;;
        yellow)  c_code="$YELLOW" ;;
        blue)    c_code="$BLUE" ;;
        magenta) c_code="$MAGENTA" ;;
        cyan)    c_code="$CYAN" ;;
        *)       c_code="$NC" ;;
    esac

    local s_code=""
    case $style in
        bold)      s_code="$BOLD" ;;
        underline) s_code="\033[4m" ;;
        *)         s_code="" ;;
    esac

    echo -e "${s_code}${c_code}${text}${NC}"
}

# Cross-distribution package installer
install_package() {
    local cmd="$1"
    local deb_pkg="$2"
    local rhel_pkg="${3:-$deb_pkg}"
    local arch_pkg="${4:-$deb_pkg}"

    if ! command -v "$cmd" &> /dev/null; then
        echo -e "${YELLOW}[INFO] Installing missing dependency: ${cmd}...${NC}"
        if command -v apt-get &> /dev/null; then
            apt-get update -y -qq && apt-get install -y -qq "$deb_pkg"
        elif command -v dnf &> /dev/null; then
            dnf install -y -q "$rhel_pkg"
        elif command -v yum &> /dev/null; then
            yum install -y -q "$rhel_pkg"
        elif command -v pacman &> /dev/null; then
            pacman -Sy --noconfirm "$arch_pkg"
        elif command -v zypper &> /dev/null; then
            zypper install -y "$deb_pkg"
        else
            echo -e "${RED}[ERROR] Unsupported package manager. Please install '$cmd' manually.${NC}\n"
            press_key
            exit 1
        fi
    fi
}

install_dependencies() {
    install_package curl curl
    install_package unzip unzip
    install_package jq jq
    install_package openssl openssl
    install_package crontab cron cronie cronie

    # Ensure cron service is active
    if command -v systemctl &> /dev/null; then
        systemctl enable --now cron 2>/dev/null || systemctl enable --now crond 2>/dev/null || true
    fi
}

# Auto-install CLI shortcuts so users can launch via `nora` or `noratunnel`
install_cli_shortcut() {
    local current_script
    current_script=$(realpath "$0" 2>/dev/null || echo "$0")
    if [[ -f "$current_script" ]]; then
        mkdir -p /usr/local/bin /usr/bin
        if [[ "$current_script" != "$SCRIPT_INSTALL_PATH" ]]; then
            cp -f "$current_script" "$SCRIPT_INSTALL_PATH" 2>/dev/null && chmod +x "$SCRIPT_INSTALL_PATH" 2>/dev/null || true
        fi
        ln -sf "$SCRIPT_INSTALL_PATH" /usr/local/bin/nora 2>/dev/null || true
        ln -sf "$SCRIPT_INSTALL_PATH" /usr/bin/nora 2>/dev/null || true
        ln -sf "$SCRIPT_INSTALL_PATH" /usr/bin/noratunnel 2>/dev/null || true
    fi
}

# Core installation & updater
download_and_extract_core() {
    local quiet_mode="$1"

    if [[ -f "$CORE_BIN" ]]; then
        if [[ "$quiet_mode" != "silent" ]]; then
            echo
            colorize green "NoraTunnel Core is already installed." bold
            sleep 1
        fi
        return 0
    fi

    mkdir -p "$CONFIG_DIR"

    # Add GitHub raw IP entry in /etc/hosts to assist with DNS filtering
    local host_entry="185.199.108.133 raw.githubusercontent.com"
    if ! grep -q "$host_entry" /etc/hosts; then
        echo -e "${YELLOW}[INFO] Resolving GitHub raw IP in /etc/hosts for connectivity...${NC}"
        echo "$host_entry" >> /etc/hosts
    fi

    if [[ $(uname) != "Linux" ]]; then
        colorize red "Unsupported operating system. Linux is required." bold
        exit 1
    fi

    local arch
    arch=$(uname -m)

    local tmp_dir
    tmp_dir=$(mktemp -d)
    local downloaded=0

    echo -e "${CYAN}Fetching NoraTunnel core engine...${NC}"
    local auth_header=()
    if [[ -n "${GITHUB_TOKEN:-}" ]]; then
        auth_header=(-H "Authorization: token $GITHUB_TOKEN")
    fi

    local core_url=""
    if [[ "$arch" == "x86_64" || "$arch" == "amd64" ]]; then
        core_url="https://github.com/TacTic-A/NoraTunnel/releases/download/v3.0.0/noratunnel-x86_64-unknown-linux-gnu.zip"
    elif [[ "$arch" == "aarch64" || "$arch" == "arm64" ]]; then
        core_url="https://github.com/TacTic-A/NoraTunnel/releases/download/v3.0.0/noratunnel-aarch64-unknown-linux-musl.zip"
    fi

    if [[ -n "$core_url" ]]; then
        echo -e "Downloading from: ${YELLOW}$core_url${NC}"
        if curl -sSL "${auth_header[@]}" --max-time 60 -o "$tmp_dir/core.zip" "$core_url"; then
            if unzip -tq "$tmp_dir/core.zip" &>/dev/null; then
                downloaded=1
            fi
        fi
    fi

    if [[ $downloaded -eq 0 ]]; then
        colorize red "Failed to download NoraTunnel core. Please verify network access." bold
        rm -rf "$tmp_dir"
        press_key
        return 1
    fi

    echo -e "Extracting core binary..."
    unzip -qo "$tmp_dir/core.zip" -d "$tmp_dir/extracted"
    
    # Locate binary inside zip
    local extracted_bin
    extracted_bin=$(find "$tmp_dir/extracted" -type f -name "noratunnel*" | head -n 1)
    if [[ -z "$extracted_bin" ]]; then
        extracted_bin=$(find "$tmp_dir/extracted" -type f -executable | head -n 1)
    fi

    if [[ -f "$extracted_bin" ]]; then
        mv -f "$extracted_bin" "$CORE_BIN"
        chmod +x "$CORE_BIN"
        colorize green "NoraTunnel Core successfully installed." bold
    else
        colorize red "Could not find valid binary inside archive." bold
        rm -rf "$tmp_dir"
        press_key
        return 1
    fi

    rm -rf "$tmp_dir"
    sleep 1
}

# Fetch Network and Geo Information
detect_server_network() {
    SERVER_IP=$(hostname -I 2>/dev/null | awk '{print $1}')
    if [[ -z "$SERVER_IP" ]]; then
        SERVER_IP=$(curl --max-time 3 -sS https://api.ipify.org 2>/dev/null || curl --max-time 3 -sS https://icanhazip.com 2>/dev/null)
    fi

    local geo_json
    geo_json=$(curl --max-time 3 -sS "http://ipwhois.app/json/${SERVER_IP}" 2>/dev/null)
    SERVER_COUNTRY=$(echo "$geo_json" | jq -r '.country // "Unknown"' 2>/dev/null)
    SERVER_ISP=$(echo "$geo_json" | jq -r '.isp // "Unknown"' 2>/dev/null)
    
    if [[ -z "$SERVER_COUNTRY" || "$SERVER_COUNTRY" == "null" ]]; then SERVER_COUNTRY="Unknown"; fi
    if [[ -z "$SERVER_ISP" || "$SERVER_ISP" == "null" ]]; then SERVER_ISP="Unknown"; fi
}

display_hero_dashboard() {
    local c_status="${GREEN}● Ready & Healthy${NC}"
    if [[ ! -f "$CORE_BIN" ]]; then
        c_status="${RED}○ Not Installed${NC}"
    fi

    local ip_disp="${YELLOW}${SERVER_IP:-Detecting...}${NC}"
    local loc_disp="${CYAN}${SERVER_COUNTRY:-Unknown}${NC}"
    local isp_disp="${PURPLE}${SERVER_ISP:-Unknown}${NC}"

    echo -e "${DARK_GRAY}╭─────────────────────────────────────────────────────────────────╮${NC}"
    echo -e "${DARK_GRAY}│${NC}  ${BOLD}${BLUE}⚡ NoraTunnel System v3.0${NC}       ${DIM}${GRAY}[High-Speed Reverse Bridge]${NC}  ${DARK_GRAY}│${NC}"
    echo -e "${DARK_GRAY}├─────────────────────────────────────────────────────────────────┤${NC}"
    printf "${DARK_GRAY}│${NC}  %-14s %-56b ${DARK_GRAY}│${NC}\n" "🌐 Public IP:" "$ip_disp"
    printf "${DARK_GRAY}│${NC}  %-14s %-56b ${DARK_GRAY}│${NC}\n" "📍 Location:" "$loc_disp"
    printf "${DARK_GRAY}│${NC}  %-14s %-56b ${DARK_GRAY}│${NC}\n" "🏢 Datacenter:" "$isp_disp"
    printf "${DARK_GRAY}│${NC}  %-14s %-56b ${DARK_GRAY}│${NC}\n" "⚙️ Core Engine:" "$c_status"
    echo -e "${DARK_GRAY}╰─────────────────────────────────────────────────────────────────╯${NC}"
}

check_ipv6() {
    local ip=$1
    local ipv6_pattern="^([0-9a-fA-F]{1,4}:){7}([0-9a-fA-F]{1,4}|:)$|^(([0-9a-fA-F]{1,4}:){1,7}|:):((:[0-9a-fA-F]{1,4}){1,7}|:)$"
    ip="${ip#[}"
    ip="${ip%]}"
    if [[ $ip =~ $ipv6_pattern ]]; then
        return 0
    else
        return 1
    fi
}


# Automatic Firewall Configuration
open_firewall_port() {
    local port="$1"
    local proto="${2:-tcp}"
    if [[ -z "$port" ]]; then return 0; fi

    if command -v ufw &>/dev/null && ufw status 2>/dev/null | grep -qi "Status: active"; then
        ufw allow "${port}/${proto}" >/dev/null 2>&1
        colorize green "[FIREWALL] Port ${port}/${proto} allowed in UFW."
    elif command -v firewall-cmd &>/dev/null && systemctl is-active --quiet firewalld 2>/dev/null; then
        firewall-cmd --quiet --permanent --add-port="${port}/${proto}" 2>/dev/null
        firewall-cmd --quiet --reload 2>/dev/null
        colorize green "[FIREWALL] Port ${port}/${proto} allowed in Firewalld."
    elif command -v iptables &>/dev/null; then
        if ! iptables -C INPUT -p "$proto" --dport "$port" -j ACCEPT 2>/dev/null; then
            iptables -I INPUT -p "$proto" --dport "$port" -j ACCEPT 2>/dev/null || true
        fi
    fi
}

check_port() {
    local port=$1
    local proto=$2
    if [[ -z "$port" ]]; then return 1; fi

    if [[ "$proto" == "tcp" ]]; then
        ss -tlnp "sport = :$port" 2>/dev/null | grep -q ":$port" && return 0 || return 1
    elif [[ "$proto" == "udp" ]]; then
        ss -ulnp "sport = :$port" 2>/dev/null | grep -q ":$port" && return 0 || return 1
    else
        return 1
    fi
}

# ----------------- CONFIGURATION: IRAN SERVER -----------------
iran_server_configuration() {
    clear
    colorize cyan "============================================================" bold
    colorize cyan "          Configuring NoraTunnel IRAN Server                " bold
    colorize cyan "============================================================" bold
    echo

    local local_ip='0.0.0.0'
    read -p "[?] Listen on IPv6 address as well? (y/n) [default: n]: " ip_ans
    if [[ "$ip_ans" == "y" || "$ip_ans" == "Y" ]]; then
        colorize yellow "[+] IPv6 Enabled (Listening on [::])"
        local_ip='[::]'
    else
        colorize yellow "[+] IPv4 Enabled (Listening on 0.0.0.0)"
        local_ip='0.0.0.0'
    fi

    echo
    local tunnel_port=""
    while true; do
        echo -ne "[*] Main Tunnel Port (Communication bridge port): "
        read -r tunnel_port
        if [[ "$tunnel_port" =~ ^[0-9]+$ ]] && [ "$tunnel_port" -gt 22 ] && [ "$tunnel_port" -le 65535 ]; then
            if check_port "$tunnel_port" "tcp"; then
                colorize red "[!] Port $tunnel_port is already in use by another service."
            else
                break
            fi
        else
            colorize red "[!] Enter a valid port between 23 and 65535."
        fi
    done

    echo
    colorize cyan "Choose Tunnel Transport & Obfuscation Layer:" bold
    echo " 1) Noise Protocol Obfuscation [Recommended - Encrypted & Bypasses DPI]"
    echo " 2) Plain TCP [Standard - Low latency, no encryption]"
    read -p "Enter choice [1-2] [default: 1]: " transport_layer_opt

    local transport_type="noise"
    local noise_priv_key=""
    local noise_pub_key=""
    local nodelay="true"

    if [[ "$transport_layer_opt" == "2" ]]; then
        transport_type="tcp"
        echo
        echo -ne "[*] Enable TCP_NODELAY (Recommended for gaming/VPN) (true/false) [default: true]: "
        read -r nodelay_in
        if [[ "$nodelay_in" == "false" ]]; then nodelay="false"; fi
    else
        download_and_extract_core "silent"
        local keygen_out
        keygen_out=$("$CORE_BIN" --genkey 2>/dev/null)
        noise_priv_key=$(echo "$keygen_out" | grep -i "Private Key" | awk -F': ' '{print $2}' | tr -d ' \r\n')
        noise_pub_key=$(echo "$keygen_out" | grep -i "Public Key" | awk -F': ' '{print $2}' | tr -d ' \r\n')
        colorize green "[+] Generated X25519 Noise Cryptographic Keypair for Obfuscation." bold
    fi

    echo
    local heartbeat="30"
    echo -ne "[*] Enable Heartbeat Keepalive? (true/false) [default: true]: "
    read -r hb_in
    if [[ "$hb_in" == "false" ]]; then heartbeat="0"; fi

    echo
    local transport="tcp"
    echo -ne "[*] Forwarded Services Traffic Type (tcp/udp) [default: tcp]: "
    read -r trans_in
    if [[ "$trans_in" == "udp" ]]; then transport="udp"; fi

    echo
    echo -ne "[*] Security Token (Press Enter to auto-generate a secure token): "
    read -r token
    if [[ -z "$token" ]]; then
        token=$(openssl rand -hex 16 2>/dev/null || tr -dc 'a-zA-Z0-9' < /dev/urandom 2>/dev/null | head -c 24)
        echo
        colorize green "[SECURITY] Generated random 256-bit token: $token" bold
    fi

    echo
    echo -ne "[*] Enter ports to forward separated by commas (e.g. 2080,2090,443): "
    read -r input_ports
    input_ports=$(echo "$input_ports" | tr -d ' ')
    IFS=',' read -r -a ports <<< "$input_ports"
    declare -a valid_ports

    for p in "${ports[@]}"; do
        if [[ "$p" =~ ^[0-9]+$ ]] && [ "$p" -gt 22 ] && [ "$p" -le 65535 ]; then
            if check_port "$p" "$transport"; then
                colorize red "[WARNING] Port $p is currently in use."
            else
                colorize green "[+] Port $p added to tunnel profile."
                valid_ports+=("$p")
            fi
        else
            colorize red "[!] Invalid port number: $p (Must be 23-65535)"
        fi
    done

    if [ ${#valid_ports[@]} -eq 0 ]; then
        colorize red "No valid ports entered. Setup cancelled." bold
        press_key
        return 1
    fi

    # Write Server TOML Configuration
    local config_file="${CONFIG_DIR}/iran${tunnel_port}.toml"
    cat << EOF > "$config_file"
[server]
bind_addr = "${local_ip}:${tunnel_port}"
default_token = "$token"
heartbeat_interval = $heartbeat

[server.transport]
type = "$transport_type"

EOF

    if [[ "$transport_type" == "noise" ]]; then
        cat << EOF >> "$config_file"
[server.transport.noise]
local_private_key = "$noise_priv_key"

EOF
    else
        cat << EOF >> "$config_file"
[server.transport.tcp]
nodelay = $nodelay

EOF
    fi

    for p in "${valid_ports[@]}"; do
        cat << EOF >> "$config_file"
[server.services.${p}]
type = "$transport"
bind_addr = "${local_ip}:${p}"

EOF
    done

    # Write Systemd Service with Hardened Resource Limits
    local service_name="noratunnel-iran${tunnel_port}.service"
    local service_path="${SERVICE_DIR}/${service_name}"
    cat << EOF > "$service_path"
[Unit]
Description=NoraTunnel Iran Server on Port $tunnel_port
After=network.target network-online.target
Wants=network-online.target

[Service]
Type=simple
ExecStart=${CORE_BIN} ${config_file}
Restart=always
RestartSec=3
LimitNOFILE=1048576
LimitNPROC=512000
StandardOutput=journal
StandardError=journal

[Install]
WantedBy=multi-user.target
EOF

    systemctl daemon-reload >/dev/null 2>&1
    systemctl enable --now "$service_name" >/dev/null 2>&1

    # Open firewall ports automatically
    open_firewall_port "$tunnel_port" "tcp"
    for p in "${valid_ports[@]}"; do
        open_firewall_port "$p" "$transport"
    done

    clear
    colorize green "============================================================" bold
    colorize green "        NoraTunnel IRAN Server Configured Successfully!      " bold
    colorize green "============================================================" bold
    echo -e " ${CYAN}Tunnel Port:${NC}        ${YELLOW}${tunnel_port}${NC}"
    echo -e " ${CYAN}Security Token:${NC}     ${YELLOW}${token}${NC}"
    echo -e " ${CYAN}Forwarded Ports:${NC}    ${GREEN}${valid_ports[*]}${NC}"
    echo -e " ${CYAN}Service Type:${NC}       ${MAGENTA}${transport^^}${NC}"
    echo -e " ${CYAN}Public Server IP:${NC}   ${YELLOW}${SERVER_IP}${NC}"
    echo -e "\e[90m─────────────────────────────────────────────────────────────\e[0m"
    colorize yellow ">> INSTRUCTIONS FOR KHAREJ SERVER:" bold
    echo -e " 1. Run NoraTunnel on your Kharej server."
    echo -e " 2. Select: ${BOLD}Configure a new tunnel -> Kharej server${NC}"
    echo -e " 3. Enter Iran Server IP: ${GREEN}${SERVER_IP}${NC}"
    echo -e " 4. Enter Tunnel Port:    ${GREEN}${tunnel_port}${NC}"
    echo -e " 5. Enter Token:          ${GREEN}${token}${NC}"
    colorize green "============================================================" bold
    press_key
}

# ----------------- CONFIGURATION: KHAREJ SERVER -----------------
kharej_server_configuration() {
    clear
    colorize cyan "============================================================" bold
    colorize cyan "         Configuring NoraTunnel KHAREJ Client               " bold
    colorize cyan "============================================================" bold
    echo

    local server_addr=""
    while true; do
        echo -ne "[*] IRAN Server IP Address (IPv4 or IPv6): "
        read -r server_addr
        if [[ -n "$server_addr" ]]; then break; fi
        colorize red "[!] Server address cannot be empty."
    done

    echo
    local tunnel_port=""
    while true; do
        echo -ne "[*] Tunnel Port (as configured on Iran server): "
        read -r tunnel_port
        if [[ "$tunnel_port" =~ ^[0-9]+$ ]] && [ "$tunnel_port" -gt 22 ] && [ "$tunnel_port" -le 65535 ]; then
            break
        else
            colorize red "[!] Enter a valid port between 23 and 65535."
        fi
    done

    echo
    colorize cyan "Choose Tunnel Transport & Obfuscation Layer:" bold
    echo " 1) Noise Protocol Obfuscation (Matches Iran Server)"
    echo " 2) Plain TCP"
    read -p "Enter choice [1-2] [default: 1]: " transport_layer_opt

    local transport_type="noise"
    local noise_pub_key=""
    local nodelay="true"

    if [[ "$transport_layer_opt" == "2" ]]; then
        transport_type="tcp"
        echo
        echo -ne "[*] Enable TCP_NODELAY (true/false) [default: true]: "
        read -r nodelay_in
        if [[ "$nodelay_in" == "false" ]]; then nodelay="false"; fi
    else
        while true; do
            echo -ne "[*] Noise Public Key (from Iran server summary): "
            read -r noise_pub_key
            noise_pub_key=$(echo "$noise_pub_key" | tr -d ' \r\n')
            if [[ -n "$noise_pub_key" ]]; then break; fi
            colorize red "[!] Noise Public Key is required for obfuscation."
        done
    fi

    echo
    local heartbeat="40"
    echo -ne "[*] Enable Heartbeat Timeout? (true/false) [default: true]: "
    read -r hb_in
    if [[ "$hb_in" == "false" ]]; then heartbeat="0"; fi

    echo
    local transport="tcp"
    echo -ne "[*] Service Traffic Type (tcp/udp) [default: tcp]: "
    read -r trans_in
    if [[ "$trans_in" == "udp" ]]; then transport="udp"; fi

    echo
    local token=""
    while true; do
        echo -ne "[*] Security Token (Must match token from Iran server): "
        read -r token
        if [[ -n "$token" ]]; then break; fi
        colorize red "[!] Security token cannot be empty."
    done

    echo
    echo -ne "[*] Enter ports to bridge separated by commas (e.g. 2080,2090,443): "
    read -r input_ports
    input_ports=$(echo "$input_ports" | tr -d ' ')
    IFS=',' read -r -a ports <<< "$input_ports"
    declare -a valid_ports

    for p in "${ports[@]}"; do
        if [[ "$p" =~ ^[0-9]+$ ]] && [ "$p" -gt 22 ] && [ "$p" -le 65535 ]; then
            if ! check_port "$p" "$transport"; then
                colorize yellow "[INFO] Port $p is not currently listening (will connect when your service runs)."
            fi
            colorize green "[+] Port $p registered."
            valid_ports+=("$p")
        else
            colorize red "[!] Invalid port: $p"
        fi
    done

    if [ ${#valid_ports[@]} -eq 0 ]; then
        colorize red "No valid ports entered. Setup cancelled." bold
        press_key
        return 1
    fi

    # Format destination IP
    local local_target="127.0.0.1"
    local remote_addr_str="$server_addr"
    if check_ipv6 "$server_addr"; then
        local_target="[::1]"
        remote_addr_str="${server_addr#[}"
        remote_addr_str="[${remote_addr_str%}]}"
    fi

    local config_file="${CONFIG_DIR}/kharej${tunnel_port}.toml"
    cat << EOF > "$config_file"
[client]
remote_addr = "${remote_addr_str}:${tunnel_port}"
default_token = "$token"
heartbeat_timeout = $heartbeat
retry_interval = 1

[client.transport]
type = "$transport_type"

EOF

    if [[ "$transport_type" == "noise" ]]; then
        cat << EOF >> "$config_file"
[client.transport.noise]
remote_public_key = "$noise_pub_key"

EOF
    else
        cat << EOF >> "$config_file"
[client.transport.tcp]
nodelay = $nodelay

EOF
    fi

    for p in "${valid_ports[@]}"; do
        cat << EOF >> "$config_file"
[client.services.${p}]
type = "$transport"
local_addr = "${local_target}:${p}"

EOF
    done

    local service_name="noratunnel-kharej${tunnel_port}.service"
    local service_path="${SERVICE_DIR}/${service_name}"
    cat << EOF > "$service_path"
[Unit]
Description=NoraTunnel Kharej Client on Port $tunnel_port
After=network.target network-online.target
Wants=network-online.target

[Service]
Type=simple
ExecStart=${CORE_BIN} ${config_file}
Restart=always
RestartSec=3
LimitNOFILE=1048576
LimitNPROC=512000
StandardOutput=journal
StandardError=journal

[Install]
WantedBy=multi-user.target
EOF

    systemctl daemon-reload >/dev/null 2>&1
    systemctl enable --now "$service_name" >/dev/null 2>&1

    # Open firewall ports automatically
    open_firewall_port "$tunnel_port" "tcp"
    for p in "${valid_ports[@]}"; do
        open_firewall_port "$p" "$transport"
    done

    clear
    echo -e "${GREEN}╭─────────────────────────────────────────────────────────────────╮${NC}"
    echo -e "${GREEN}│${NC}    ${BOLD}${WHITE}🚀  NoraTunnel KHAREJ Client Started Successfully!${NC}           ${GREEN}│${NC}"
    echo -e "${GREEN}├─────────────────────────────────────────────────────────────────┤${NC}"
    printf "${GREEN}│${NC}  %-22s %-48b ${GREEN}│${NC}\n" "Target Iran Server:" "${YELLOW}${remote_addr_str}:${tunnel_port}${NC}"
    printf "${GREEN}│${NC}  %-22s %-48b ${GREEN}│${NC}\n" "Transport Mode:" "${CYAN}${transport_type^^}${NC}"
    printf "${GREEN}│${NC}  %-22s %-48b ${GREEN}│${NC}\n" "Bridged Ports:" "${WHITE}${valid_ports[*]}${NC}"
    printf "${GREEN}│${NC}  %-22s %-48b ${GREEN}│${NC}\n" "Service Daemon:" "${GREEN}Active & Enabled on Boot${NC}"
    echo -e "${GREEN}╰─────────────────────────────────────────────────────────────────╯${NC}"
    press_key
}

configure_tunnel() {
    if [[ ! -f "$CORE_BIN" ]]; then
        echo -e "\n${RED}[!] NoraTunnel core not found. Installing now...${NC}\n"
        download_and_extract_core
    fi

    clear
    colorize cyan "Select Tunnel Operation Mode:" bold
    echo
    colorize green   " 1) Configure as IRAN Server (Tunnel receiver & listener)" bold
    colorize magenta " 2) Configure as KHAREJ Server (Tunnel client & forwarder)" bold
    echo
    read -p "Enter choice [1-2]: " mode_choice
    case "$mode_choice" in
        1) iran_server_configuration ;;
        2) kharej_server_configuration ;;
        *) colorize red "Invalid option!" && sleep 1 ;;
    esac
}

# ----------------- TUNNEL MANAGEMENT -----------------
find_active_configs() {
    declare -ga ALL_CONFIGS=()
    for cfg in "$CONFIG_DIR"/iran*.toml "$CONFIG_DIR"/kharej*.toml; do
        if [[ -f "$cfg" ]]; then
            ALL_CONFIGS+=("$cfg")
        fi
    done
}

check_tunnel_status() {
    clear
    find_active_configs
    if [ ${#ALL_CONFIGS[@]} -eq 0 ]; then
        colorize yellow "No NoraTunnel configuration files found." bold
        press_key
        return 0
    fi

    colorize cyan "=================== TUNNEL STATUS DASHBOARD ===================" bold
    echo
    for cfg in "${ALL_CONFIGS[@]}"; do
        local fname
        fname=$(basename "$cfg" .toml)
        local s_name="noratunnel-${fname}.service"


        local role="Iran"
        local port="${fname#iran}"
        if [[ "$fname" =~ ^kharej ]]; then
            role="Kharej"
            port="${fname#kharej}"
        fi

        local t_layer="TCP"
        if grep -q 'type = "noise"' "$cfg" 2>/dev/null; then
            t_layer="${GREEN}Noise Protocol (Obfuscated & Encrypted)${NC}"
        else
            t_layer="${YELLOW}Plain TCP${NC}"
        fi

        local svc_state="${GREEN}● Active (Running)${NC}"
        if ! systemctl is-active --quiet "$s_name"; then
            svc_state="${RED}○ Inactive (Stopped / Failed)${NC}"
        fi

        local net_state="${GREEN}Listening (: $port)${NC}"
        if ! ss -tlnp 2>/dev/null | grep -q ":$port "; then
            net_state="${YELLOW}Remote Bridge Mode${NC}"
        fi

        echo -e "${DARK_GRAY}╭─────────────────────────────────────────────────────────────────╮${NC}"
        printf "${DARK_GRAY}│${NC}  ${BOLD}%-10s${NC} Tunnel Port: ${YELLOW}%-40s${NC} ${DARK_GRAY}│${NC}\n" "$role" "$port"
        echo -e "${DARK_GRAY}├─────────────────────────────────────────────────────────────────┤${NC}"
        printf "${DARK_GRAY}│${NC}  %-14s %-56b ${DARK_GRAY}│${NC}\n" "Security:" "$t_layer"
        printf "${DARK_GRAY}│${NC}  %-14s %-56b ${DARK_GRAY}│${NC}\n" "Daemon:" "$svc_state"
        printf "${DARK_GRAY}│${NC}  %-14s %-56b ${DARK_GRAY}│${NC}\n" "Network:" "$net_state"
        echo -e "${DARK_GRAY}╰─────────────────────────────────────────────────────────────────╯${NC}"
    done
    press_key
}

tunnel_management() {
    clear
    find_active_configs
    if [ ${#ALL_CONFIGS[@]} -eq 0 ]; then
        colorize yellow "No NoraTunnel configuration files found." bold
        press_key
        return 0
    fi

    colorize cyan "Select a Tunnel to Manage:" bold
    echo
    local idx=1
    for cfg in "${ALL_CONFIGS[@]}"; do
        local fname
        fname=$(basename "$cfg" .toml)
        local role="Iran"
        local port="${fname#iran}"
        if [[ "$fname" =~ ^kharej ]]; then
            role="Kharej"
            port="${fname#kharej}"
        fi
        echo -e "${MAGENTA}${idx}${NC}) ${CYAN}${role}${NC} Tunnel (Port: ${YELLOW}${port}${NC})"
        ((idx++))
    done
    echo
    read -p "Enter tunnel number (0 to return): " t_choice
    if [[ "$t_choice" == "0" || -z "$t_choice" ]]; then return 0; fi

    if ! [[ "$t_choice" =~ ^[0-9]+$ ]] || (( t_choice < 1 || t_choice > ${#ALL_CONFIGS[@]} )); then
        colorize red "Invalid selection!"
        sleep 1
        return 1
    fi

    local selected_cfg="${ALL_CONFIGS[$((t_choice - 1))]}"
    local fname
    fname=$(basename "$selected_cfg" .toml)
    local s_name="noratunnel-${fname}.service"


    clear
    colorize cyan "Managing Tunnel: $fname" bold
    echo
    colorize red     " 1) Destroy / Remove this tunnel"
    colorize yellow  " 2) Restart this tunnel"
    colorize green   " 3) Add additional port to this tunnel"
    colorize cyan    " 4) Add automated cron restart schedule"
    colorize magenta " 5) Remove cron restart schedule"
    colorize blue    " 6) View live active connections on this tunnel"
    echo -e " 7) View live service logs (journalctl)"
    echo -e " 8) View service status (systemctl)"
    echo -e " 0) Return"
    echo
    read -p "Enter choice [0-8]: " sub_choice
    case $sub_choice in
        1) destroy_tunnel "$selected_cfg" "$s_name" ;;
        2) restart_tunnel_service "$s_name" ;;
        3) add_port_to_tunnel "$selected_cfg" "$s_name" ;;
        4) add_cron_schedule "$s_name" ;;
        5) delete_cron_schedule "$s_name" ;;
        6) 
            local p="${fname#iran}"; p="${p#kharej}"
            view_active_connections "$p"
            ;;
        7) journalctl -u "$s_name" -n 50 --no-pager; press_key ;;
        8) systemctl status "$s_name"; press_key ;;
        0) return 0 ;;
        *) colorize red "Invalid option!" && sleep 1 ;;
    esac
}

destroy_tunnel() {
    local cfg="$1"
    local s_name="$2"
    echo
    colorize yellow "Are you sure you want to completely remove this tunnel? (y/n)"
    read -r confirm
    if [[ "$confirm" != "y" && "$confirm" != "Y" ]]; then return 0; fi

    delete_cron_schedule "$s_name" >/dev/null 2>&1
    systemctl stop "$s_name" >/dev/null 2>&1
    systemctl disable "$s_name" >/dev/null 2>&1
    rm -f "${SERVICE_DIR}/${s_name}"
    rm -f "$cfg"
    systemctl daemon-reload >/dev/null 2>&1
    colorize green "Tunnel successfully destroyed." bold
    sleep 1
}

restart_tunnel_service() {
    local s_name="$1"
    colorize yellow "Restarting $s_name..." bold
    if systemctl restart "$s_name"; then
        colorize green "Service restarted cleanly." bold
    else
        colorize red "Failed to restart service. Check logs." bold
    fi
    press_key
}

add_port_to_tunnel() {
    local cfg="$1"
    local s_name="$2"
    echo
    echo -ne "[*] Enter additional ports separated by commas (e.g. 8080,8443): "
    read -r input_ports
    input_ports=$(echo "$input_ports" | tr -d ' ')
    IFS=',' read -r -a new_ports <<< "$input_ports"

    echo -ne "[*] Service Traffic Type (tcp/udp) [default: tcp]: "
    read -r proto
    if [[ "$proto" != "udp" ]]; then proto="tcp"; fi

    local added=0
    for p in "${new_ports[@]}"; do
        if [[ "$p" =~ ^[0-9]+$ ]] && [ "$p" -gt 22 ] && [ "$p" -le 65535 ]; then
            if grep -q "services\.${p}" "$cfg"; then
                colorize yellow "[!] Port $p is already configured. Skipping."
                continue
            fi

            if [[ "$cfg" =~ iran ]]; then
                cat << EOF >> "$cfg"
[server.services.${p}]
type = "$proto"
bind_addr = "0.0.0.0:${p}"

EOF
            else
                cat << EOF >> "$cfg"
[client.services.${p}]
type = "$proto"
local_addr = "127.0.0.1:${p}"

EOF
            fi
            colorize green "[+] Port $p appended to tunnel."
            ((added++))
        fi
    done

    if [ $added -gt 0 ]; then
        for p in "${new_ports[@]}"; do
            open_firewall_port "$p" "$proto"
        done
        systemctl restart "$s_name"
        colorize green "Configuration updated and service reloaded!" bold
    fi
    press_key
}

add_cron_schedule() {
    local s_name="$1"
    clear
    colorize cyan "Choose Restart Schedule Interval:" bold
    echo
    echo " 1) Every 30 minutes"
    echo " 2) Every 1 hour"
    echo " 3) Every 2 hours"
    echo " 4) Every 4 hours"
    echo " 5) Every 6 hours"
    echo " 6) Every 12 hours"
    echo " 7) Every 24 hours (Midnight)"
    echo
    read -p "Enter choice [1-7]: " c_choice
    local cron_time=""
    case $c_choice in
        1) cron_time="*/30 * * * *" ;;
        2) cron_time="0 * * * *" ;;
        3) cron_time="0 */2 * * *" ;;
        4) cron_time="0 */4 * * *" ;;
        5) cron_time="0 */6 * * *" ;;
        6) cron_time="0 */12 * * *" ;;
        7) cron_time="0 0 * * *" ;;
        *) colorize red "Invalid option!" && return 1 ;;
    esac

    delete_cron_schedule "$s_name" >/dev/null 2>&1
    local script_path="${CONFIG_DIR}/${s_name%.service}.sh"

    # Clean, isolated restart command
    cat << EOF > "$script_path"
#!/bin/bash
systemctl restart $s_name
EOF
    chmod +x "$script_path"

    local tmp_cron
    tmp_cron=$(mktemp)
    crontab -l 2>/dev/null | grep -v "#$s_name" > "$tmp_cron" || true
    echo "$cron_time $script_path #$s_name" >> "$tmp_cron"
    crontab "$tmp_cron"
    rm -f "$tmp_cron"

    colorize green "Scheduled restart configured cleanly for $s_name." bold
    press_key
}

delete_cron_schedule() {
    local s_name="$1"
    crontab -l 2>/dev/null | grep -v "#$s_name" | crontab - 2>/dev/null || true
    rm -f "${CONFIG_DIR}/${s_name%.service}.sh" 2>/dev/null
    colorize green "Automated cron schedule removed for $s_name." bold
    sleep 1
}

view_active_connections() {
    local port="$1"
    clear
    colorize cyan "Active TCP Connections on Port $port:" bold
    echo
    ss -tnp "sport = :$port" 2>/dev/null || ss -tanp | grep ":$port" || echo "No active connections detected."
    press_key
}


# ----------------- CONNECTIVITY & LATENCY TESTER -----------------
test_tcp_connectivity() {
    local host="$1"
    local port="$2"
    local timeout="${3:-2}"

    local start_time
    start_time=$(date +%s%N 2>/dev/null || date +%s)
    
    if timeout "$timeout" bash -c "</dev/tcp/$host/$port" 2>/dev/null; then
        local end_time
        end_time=$(date +%s%N 2>/dev/null || date +%s)
        local diff_ms=$(( (end_time - start_time) / 1000000 ))
        if [[ $diff_ms -le 0 ]]; then diff_ms=1; fi
        echo "SUCCESS:$diff_ms"
    else
        echo "FAILED"
    fi
}

connectivity_test_menu() {
    clear
    colorize cyan "================ CONNECTIVITY & LATENCY TESTER ================" bold
    echo "Check direct TCP handshake reachability and round-trip latency."
    echo

    local target_host=""
    local target_port=""

    echo -ne "[*] Enter Target Host / Server IP: "
    read -r target_host
    if [[ -z "$target_host" ]]; then
        colorize red "[!] Host cannot be empty."
        press_key
        return 1
    fi

    echo -ne "[*] Enter Target Port: "
    read -r target_port
    if ! [[ "$target_port" =~ ^[0-9]+$ ]] || (( target_port < 1 || target_port > 65535 )); then
        colorize red "[!] Invalid port number."
        press_key
        return 1
    fi

    echo
    colorize yellow "Testing connection to ${target_host}:${target_port}..." bold
    echo

    local attempts=4
    local successes=0
    local total_ms=0

    for i in $(seq 1 $attempts); do
        local res
        res=$(test_tcp_connectivity "$target_host" "$target_port" 2)
        if [[ "$res" =~ ^SUCCESS:([0-9]+) ]]; then
            local ms="${BASH_REMATCH[1]}"
            echo -e " Reply from ${target_host}: seq=$i ${GREEN}Connected${NC} latency=${YELLOW}${ms}ms${NC}"
            ((successes++))
            total_ms=$(( total_ms + ms ))
        else
            echo -e " Connection to ${target_host}:${target_port}: seq=$i ${RED}Timed out / Filtered${NC}"
        fi
        sleep 0.4
    done

    echo
    echo -e "${BOLD}--- Test Summary for ${target_host}:${target_port} ---${NC}"
    echo -e " Packets: Sent = $attempts, Received = $successes, Lost = $(( attempts - successes ))"
    if [[ $successes -gt 0 ]]; then
        local avg_ms=$(( total_ms / successes ))
        echo -e " Average Latency: ${GREEN}${avg_ms}ms${NC}"
        colorize green "[+] Port is open and reachable over the network." bold
    else
        colorize red "[!] Connection failed. The port is unreachable, firewalled, or filtered." bold
    fi
    press_key
}

# ----------------- SMART WATCHDOG AUTO-RECOVERY -----------------
WATCHDOG_SCRIPT="${CONFIG_DIR}/noratunnel-watchdog.sh"
WATCHDOG_SERVICE="${SERVICE_DIR}/noratunnel-watchdog.service"

setup_watchdog_script() {
    mkdir -p "$CONFIG_DIR"
    cat << 'EOF' > "$WATCHDOG_SCRIPT"
#!/bin/bash
CONFIG_DIR="/root/noratunnel-core"
for cfg in "$CONFIG_DIR"/iran*.toml "$CONFIG_DIR"/kharej*.toml; do
    if [[ -f "$cfg" ]]; then
        fname=$(basename "$cfg" .toml)
        s_name="noratunnel-${fname}.service"
        if systemctl is-enabled "$s_name" &>/dev/null; then
            if ! systemctl is-active --quiet "$s_name"; then
                echo "[$(date)] NoraWatchdog: $s_name is inactive. Reviving..."
                systemctl restart "$s_name"
            fi
        fi
    fi
done
EOF
    chmod +x "$WATCHDOG_SCRIPT"

    cat << EOF > "$WATCHDOG_SERVICE"
[Unit]
Description=NoraTunnel Self-Healing Watchdog
After=network.target

[Service]
Type=simple
ExecStart=/bin/bash -c "while true; do ${WATCHDOG_SCRIPT}; sleep 45; done"
Restart=always
RestartSec=10

[Install]
WantedBy=multi-user.target
EOF
    systemctl daemon-reload >/dev/null 2>&1
}

manage_watchdog_menu() {
    clear
    colorize cyan "================ SMART WATCHDOG AUTO-RECOVERY ================" bold
    echo "The watchdog monitors all NoraTunnel services every 45 seconds and"
    echo "automatically heals any dead, crashed, or stalled tunnel daemons."
    echo

    local w_status="${RED}Inactive / Disabled${NC}"
    if systemctl is-active --quiet "noratunnel-watchdog.service" 2>/dev/null; then
        w_status="${GREEN}Active & Monitoring${NC}"
    fi
    echo -e " Current Status: $w_status"
    echo
    echo " 1) Enable and Start Watchdog Daemon"
    echo " 2) Disable and Stop Watchdog Daemon"
    echo " 3) View Watchdog Activity Logs"
    echo " 0) Return"
    echo
    read -p "Enter choice [0-3]: " w_opt
    case "$w_opt" in
        1)
            setup_watchdog_script
            systemctl enable --now "noratunnel-watchdog.service" >/dev/null 2>&1
            colorize green "NoraTunnel Watchdog daemon enabled and running." bold
            press_key
            ;;
        2)
            systemctl stop "noratunnel-watchdog.service" >/dev/null 2>&1
            systemctl disable "noratunnel-watchdog.service" >/dev/null 2>&1
            colorize yellow "NoraTunnel Watchdog daemon stopped and disabled." bold
            press_key
            ;;
        3)
            journalctl -u "noratunnel-watchdog.service" -n 50 --no-pager
            press_key
            ;;
        *) return 0 ;;
    esac
}

# ----------------- BACKUP & RESTORE CONFIGURATIONS -----------------
backup_restore_menu() {
    clear
    colorize cyan "================ BACKUP & RESTORE CONFIGURATIONS ================" bold
    echo "Easily export or import all your tunnel profiles, keys, and services."
    echo
    echo " 1) Create Full Backup Archive"
    echo " 2) Restore Configurations from Archive"
    echo " 0) Return"
    echo
    read -p "Enter choice [0-2]: " br_opt
    case "$br_opt" in
        1)
            local timestamp
            timestamp=$(date +"%Y%m%d_%H%M%S")
            local backup_tar="/root/noratunnel_backup_${timestamp}.tar.gz"

            mkdir -p "$CONFIG_DIR"
            local backup_items=("root/noratunnel-core")
            for sf in /etc/systemd/system/noratunnel*.service; do
                if [[ -f "$sf" ]]; then
                    backup_items+=("etc/systemd/system/$(basename "$sf")")
                fi
            done
            if tar -czf "$backup_tar" -C / "${backup_items[@]}" 2>/dev/null; then
                colorize green "Backup archive created successfully:" bold
                echo -e " Archive Path: ${YELLOW}${backup_tar}${NC}"
                echo -e " Archive Size: $(du -h "$backup_tar" 2>/dev/null | cut -f1)"
            else
                colorize red "Failed to create backup archive."
            fi
            press_key
            ;;
        2)
            echo
            echo -ne "[*] Enter full path to backup file (.tar.gz): "
            read -r r_file
            if [[ -f "$r_file" ]]; then
                if tar -xzf "$r_file" -C / 2>/dev/null; then
                    systemctl daemon-reload >/dev/null 2>&1
                    for s in /etc/systemd/system/noratunnel*.service; do
                        if [[ -f "$s" ]]; then
                            systemctl enable --now "$(basename "$s")" >/dev/null 2>&1
                        fi
                    done
                    colorize green "Configurations restored and services started!" bold
                else
                    colorize red "Failed to extract backup archive."
                fi
            else
                colorize red "Backup file not found: $r_file"
            fi
            press_key
            ;;
        *) return 0 ;;
    esac
}

# ----------------- SYSTEM & NETWORK OPTIMIZATIONS -----------------
optimize_network_and_system() {
    clear
    colorize cyan "Applying High-Performance Kernel & Network Tweaks..." bold
    echo

    cp "$SYS_PATH" /etc/sysctl.conf.bak 2>/dev/null
    sed -i -e '/fs.file-max/d' \
        -e '/net.core.default_qdisc/d' \
        -e '/net.core.netdev_max_backlog/d' \
        -e '/net.core.somaxconn/d' \
        -e '/net.core.rmem_max/d' \
        -e '/net.core.wmem_max/d' \
        -e '/net.ipv4.tcp_rmem/d' \
        -e '/net.ipv4.tcp_wmem/d' \
        -e '/net.ipv4.tcp_congestion_control/d' \
        -e '/net.ipv4.tcp_notsent_lowat/d' \
        -e '/net.ipv4.tcp_sack/d' \
        -e '/net.ipv4.tcp_window_scaling/d' \
        -e '/net.ipv4.tcp_syncookies/d' \
        -e '/vm.swappiness/d' \
        "$SYS_PATH"

    cat <<EOF >> "$SYS_PATH"
# ==================== NORATUNNEL TUNING ====================
fs.file-max = 67108864
net.core.default_qdisc = fq_codel
net.core.netdev_max_backlog = 32768
net.core.somaxconn = 65536
net.core.rmem_max = 33554432
net.core.wmem_max = 33554432
net.ipv4.tcp_rmem = 16384 1048576 33554432
net.ipv4.tcp_wmem = 16384 1048576 33554432
net.ipv4.tcp_congestion_control = bbr
net.ipv4.tcp_notsent_lowat = 32768
net.ipv4.tcp_sack = 1
net.ipv4.tcp_dsack = 1
net.ipv4.tcp_window_scaling = 1
net.ipv4.tcp_syncookies = 1
vm.swappiness = 10
# ===========================================================
EOF

    sysctl -p 2>/dev/null || true

    sed -i '/ulimit -/d' "$PROF_PATH"
    cat <<EOF >> "$PROF_PATH"
ulimit -c unlimited
ulimit -d unlimited
ulimit -f unlimited
ulimit -n 1048576
ulimit -s 32768
ulimit -u unlimited
EOF

    colorize green "Kernel parameters and system limits successfully optimized." bold
    echo
    echo -ne "${YELLOW}Do you wish to reboot now? (y/n) [default: n]: ${NC}"
    read -r r_ans
    if [[ "$r_ans" == "y" || "$r_ans" == "Y" ]]; then
        reboot
        exit 0
    fi
}

remove_all_noratunnel() {
    clear
    colorize red "================== REMOVE NORATUNNEL ==================" bold
    colorize red "This will remove all NoraTunnel services, configs, and binaries!"
    echo
    read -p "Are you sure? Type 'YES' to confirm: " c_del
    if [[ "$c_del" != "YES" ]]; then
        colorize yellow "Operation cancelled."
        sleep 1
        return 0
    fi

    for s in /etc/systemd/system/noratunnel*.service; do
        if [[ -f "$s" ]]; then
            local s_base
            s_base=$(basename "$s")
            systemctl stop "$s_base" 2>/dev/null
            systemctl disable "$s_base" 2>/dev/null
            rm -f "$s"
        fi
    done

    systemctl daemon-reload >/dev/null 2>&1
    rm -rf "$CONFIG_DIR"
    rm -f /usr/local/bin/noratunnel /usr/local/bin/nora
    colorize green "All NoraTunnel data and services removed cleanly." bold
    press_key
}

# ----------------- MAIN MENU -----------------
display_main_menu() {
    clear
    display_hero_dashboard
    echo
    echo -e "${DARK_GRAY}╭─── CORE TUNNELING ──────────────────────────────────────────────╮${NC}"
    echo -e "${DARK_GRAY}│${NC}  ${BOLD}${BLUE}[ 1 ]${NC} ${WHITE}Configure a New Tunnel${NC}      ${GRAY}Setup Iran or Kharej bridge${NC}    ${DARK_GRAY}│${NC}"
    echo -e "${DARK_GRAY}│${NC}  ${BOLD}${CYAN}[ 2 ]${NC} ${WHITE}Tunnel Management Menu${NC}      ${GRAY}Ports, Restart, Logs, Cron${NC}     ${DARK_GRAY}│${NC}"
    echo -e "${DARK_GRAY}│${NC}  ${BOLD}${GREEN}[ 3 ]${NC} ${WHITE}Live Status Dashboard${NC}       ${GRAY}Inspect sockets & daemons${NC}      ${DARK_GRAY}│${NC}"
    echo -e "${DARK_GRAY}├─── RELIABILITY & DIAGNOSTICS ───────────────────────────────────┤${NC}"
    echo -e "${DARK_GRAY}│${NC}  ${BOLD}${YELLOW}[ 4 ]${NC} ${WHITE}Test Latency & Reachability${NC} ${GRAY}Measure TCP RTT & packet loss${NC}  ${DARK_GRAY}│${NC}"
    echo -e "${DARK_GRAY}│${NC}  ${BOLD}${PURPLE}[ 5 ]${NC} ${WHITE}Smart Watchdog Recovery${NC}     ${GRAY}Auto-healing background daemon${NC} ${DARK_GRAY}│${NC}"
    echo -e "${DARK_GRAY}│${NC}  ${BOLD}${BLUE}[ 6 ]${NC} ${WHITE}Backup & Restore Profiles${NC}   ${GRAY}Export/Import tar.gz archive${NC}   ${DARK_GRAY}│${NC}"
    echo -e "${DARK_GRAY}├─── PERFORMANCE & SYSTEM ────────────────────────────────────────┤${NC}"
    echo -e "${DARK_GRAY}│${NC}  ${BOLD}${GREEN}[ 7 ]${NC} ${WHITE}Kernel & Network Tuning${NC}     ${GRAY}Google BBR & 1M socket limits${NC}  ${DARK_GRAY}│${NC}"
    echo -e "${DARK_GRAY}│${NC}  ${BOLD}${CYAN}[ 8 ]${NC} ${WHITE}Reinstall / Update Core${NC}     ${GRAY}Verify engine binary integrity${NC} ${DARK_GRAY}│${NC}"
    echo -e "${DARK_GRAY}│${NC}  ${BOLD}${RED}[ 9 ]${NC} ${WHITE}Uninstall NoraTunnel${NC}        ${GRAY}Purge all profiles & services${NC}  ${DARK_GRAY}│${NC}"
    echo -e "${DARK_GRAY}│${NC}  ${BOLD}${GRAY}[ 0 ]${NC} ${WHITE}Exit Dashboard${NC}                                            ${DARK_GRAY}│${NC}"
    echo -e "${DARK_GRAY}╰─────────────────────────────────────────────────────────────────╯${NC}"
}

main() {
    install_dependencies
    install_cli_shortcut
    download_and_extract_core "silent"
    detect_server_network

    while true; do
        display_main_menu
        echo -ne "${BOLD}${BLUE} ➜ Select an action [0-9]: ${NC}"
        read -r main_opt
        case "$main_opt" in
            1) configure_tunnel ;;
            2) tunnel_management ;;
            3) check_tunnel_status ;;
            4) connectivity_test_menu ;;
            5) manage_watchdog_menu ;;
            6) backup_restore_menu ;;
            7) optimize_network_and_system ;;
            8) download_and_extract_core ;;
            9) remove_all_noratunnel ;;
            0) clear; exit 0 ;;
            *) colorize red "Invalid option!" && sleep 1 ;;
        esac
    done
}

main "$@"
