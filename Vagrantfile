# Vagrantfile — Arch Linux VirtualBox VM for running icedtea-wm as a real
# compositor process, visible in the VirtualBox GUI window.
#
# The VM uses the VMSVGA graphics controller, which the Linux guest exposes
# as a DRM/KMS device via the `vmwgfx` kernel driver. wlroots' DRM backend
# drives that device directly, so the compositor's actual output is what the
# VirtualBox console window displays — no nested session involved.
#
# Usage:
#   vagrant up            # first boot: provisions + builds (takes a while)
#   vagrant rsync         # push local source changes into the VM
#   vagrant ssh -c 'cd icedtea-wm && cargo build --release'
#   # then, in the VirtualBox GUI window (auto-logged-in tty1):
#   icedtea               # starts the compositor on the VM's DRM device
#
# Click into the GUI window to give it keyboard/mouse; Host key (Right Ctrl
# by default) releases the grab.

Vagrant.configure("2") do |config|
  # generic/arch, not archlinux/archlinux: the official box stopped shipping
  # a virtualbox provider (libvirt only). The generic box is an older Arch
  # snapshot; provisioning refreshes the keyring and full-upgrades it.
  config.vm.box = "generic/arch"
  config.vm.hostname = "icedtea-vm"

  # rsync (one-way, host -> guest) instead of vboxsf: cargo builds on a
  # vboxsf mount are slow and flaky, and the guest owning its own copy under
  # ~vagrant keeps target/ native. Re-push edits with `vagrant rsync` or keep
  # `vagrant rsync-auto` running.
  config.vm.synced_folder ".", "/home/vagrant/icedtea-wm",
    type: "rsync",
    rsync__args: ["--archive", "--delete"],
    rsync__exclude: [".git/", "target/", ".remember/", ".superpowers/", ".claude/"]

  config.vm.provider "virtualbox" do |vb|
    vb.name = "icedtea-wm"
    vb.gui = true
    vb.memory = 4096
    vb.cpus = 4
    vb.customize ["modifyvm", :id, "--graphicscontroller", "vmsvga"]
    vb.customize ["modifyvm", :id, "--vram", "128"]
    vb.customize ["modifyvm", :id, "--accelerate-3d", "on"]
  end

  # Root phase: packages, seatd, groups, autologin, helper script.
  config.vm.provision "system", type: "shell", path: "vagrant/provision-system.sh"

  # User phase: rust toolchain + first build, as the vagrant user.
  config.vm.provision "build", type: "shell",
    path: "vagrant/provision-build.sh", privileged: false
end
