terraform {
  required_version = ">= 1.5"
  required_providers {
    google = {
      source  = "hashicorp/google"
      version = "~> 5.0"
    }
  }
}

provider "google" {
  project = var.project_id
  region  = var.region
  zone    = var.zone
}

# ---------- Network ----------

resource "google_compute_network" "amux" {
  name                    = "amux-net"
  auto_create_subnetworks = false
}

resource "google_compute_subnetwork" "amux" {
  name          = "amux-subnet"
  ip_cidr_range = "10.10.0.0/24"
  network       = google_compute_network.amux.id
  region        = var.region
}

# Allow Tailscale WireGuard from anywhere (needed for direct peer connections)
resource "google_compute_firewall" "tailscale" {
  name    = "amux-allow-tailscale"
  network = google_compute_network.amux.id

  allow {
    protocol = "udp"
    ports    = ["41641"]
  }

  source_ranges = ["0.0.0.0/0"]
  target_tags   = ["amux"]
}

# Block all other inbound (default GCP behavior, explicit for clarity)
resource "google_compute_firewall" "deny_inbound" {
  name     = "amux-deny-inbound"
  network  = google_compute_network.amux.id
  priority = 1000

  deny {
    protocol = "tcp"
  }
  deny {
    protocol = "udp"
  }

  source_ranges = ["0.0.0.0/0"]
  target_tags   = ["amux"]
}

# ---------- VM ----------

resource "google_compute_instance" "amux" {
  name         = "amux-dev"
  machine_type = var.machine_type
  zone         = var.zone

  tags = ["amux"]

  boot_disk {
    initialize_params {
      image = "ubuntu-os-cloud/ubuntu-2204-lts"
      size  = var.disk_size_gb
      type  = "pd-standard"
    }
  }

  network_interface {
    subnetwork = google_compute_subnetwork.amux.id

    # Ephemeral public IP — required for internet access during setup
    # (apt-get, tailscale install, npm). Inbound is firewall-restricted.
    access_config {}
  }

  # Minimal service account
  service_account {
    scopes = ["cloud-platform"]
  }

  metadata = {
    enable-oslogin = "TRUE"
  }

  metadata_startup_script = templatefile("${path.module}/setup.sh", {
    tailscale_auth_key = var.tailscale_auth_key
  })

  scheduling {
    preemptible       = false
    automatic_restart = true
  }

  allow_stopping_for_update = true
}
