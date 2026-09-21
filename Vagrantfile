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
#   vagrant provision     # reinstall the session artifacts + rebuild
#   vagrant/verify.sh     # screenshot-assert the desktop is up
#
# Boot lands on the wdm greeter; logging in runs the repo's own icedtea session
# (`icedtea-session-start --wait` starts `icedtea-session.target`). There is no
# launcher to run by hand. Click into the GUI window to give it keyboard/mouse;
# Host key (Right Ctrl by default) releases the grab.

# The guest has no `.git` (rsync excludes it), so stamp the host revision here
# and hand it to the build provisioner for BUILD_INFO.
build_rev = `git -C "#{File.dirname(__FILE__)}" rev-parse --short HEAD 2>/dev/null`.strip
build_rev = "unknown" if build_rev.empty?

Vagrant.configure("2") do |config|
  # generic/arch, not archlinux/archlinux: the official box stopped shipping
  # a virtualbox provider (libvirt only). The generic box is an older Arch
  # snapshot; provisioning refreshes the keyring and full-upgrades it.
  config.vm.box = "generic/arch"
  config.vm.hostname = "icedtea-vm"

  # rsync (one-way, host -> guest). `session/`, the unit files and the
  # wallpaper asset ride this same tree: provisioning installs them from the
  # guest's copy, so the VM's session is always the checked-in one.
  config.vm.synced_folder ".", "/home/vagrant/icedtea-wm",
    type: "rsync",
    rsync__args: ["--archive", "--delete"],
    rsync__exclude: [".git/", "target/", ".remember/", ".superpowers/", ".claude/", ".vagrant/", ".worktrees/"]

  config.vm.provider "virtualbox" do |vb|
    vb.name = "icedtea-wm"
    vb.gui = true
    vb.memory = 4096
    vb.cpus = 4
    vb.customize ["modifyvm", :id, "--graphicscontroller", "vmsvga"]
    vb.customize ["modifyvm", :id, "--vram", "128"]
    vb.customize ["modifyvm", :id, "--accelerate-3d", "on"]
  end

  # Root phase: packages, seatd, groups, session units/asset, wdm.
  config.vm.provision "system", type: "shell", path: "vagrant/provision-system.sh"

  # User phase: rust toolchain + release build, as the vagrant user. The host
  # revision rides the provisioner environment down to BUILD_INFO.
  config.vm.provision "build", type: "shell",
    path: "vagrant/provision-build.sh", privileged: false,
    env: { "ICEDTEA_BUILD_REV" => build_rev }
end
