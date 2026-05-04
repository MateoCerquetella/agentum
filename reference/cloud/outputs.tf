output "public_ip" {
  description = "Ephemeral public IP of the amux VM (for internet access during setup only)"
  value       = google_compute_instance.amux.network_interface[0].access_config[0].nat_ip
}

output "vm_name" {
  description = "VM instance name"
  value       = google_compute_instance.amux.name
}

output "zone" {
  description = "VM zone"
  value       = google_compute_instance.amux.zone
}

output "tailscale_url" {
  description = "amux dashboard URL once Tailscale is connected"
  value       = "https://amux-cloud.<tailnet>.ts.net:8822"
}

output "ssh_command" {
  description = "SSH via Tailscale (after VM connects)"
  value       = "ssh root@amux-cloud"
}
