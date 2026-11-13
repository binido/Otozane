# Binary RPM built by scripts/package-linux.sh from a staged payload.
%global debug_package %{nil}
%global __os_install_post %{nil}
# The bundled Hush library is private; do not advertise it as a system library.
%global __provides_exclude_from ^%{_prefix}/lib/otozane/.*$

Name:           otozane
Version:        %{oz_version}
Release:        1
Summary:        Real-time microphone processor for PipeWire
License:        MIT
URL:            https://github.com/binido/Otozane

# Linked libraries come from AutoReq. These are loaded at runtime.
Requires:       (libEGL.so.1()(64bit) or libGL.so.1()(64bit))
Requires:       libxkbcommon.so.0()(64bit)
Recommends:     pipewire
Recommends:     wireplumber
Recommends:     libwayland-client.so.0()(64bit)
Recommends:     libwayland-egl.so.1()(64bit)
Recommends:     xdg-desktop-portal
Recommends:     hicolor-icon-theme

%description
Captures a microphone, runs it through an effect chain and publishes
the result as the virtual input device "Otozane Microphone".

%install
cp -a %{oz_payload}/. %{buildroot}/

%files
%{_bindir}/otozane
%{_bindir}/otozaned
%{_prefix}/lib/otozane/
%{_datadir}/applications/otozane.desktop
%{_datadir}/icons/hicolor/scalable/apps/otozane.svg
%license %{_datadir}/licenses/otozane/
%doc %{_datadir}/doc/otozane/README.md
