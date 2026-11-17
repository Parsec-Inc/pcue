# Empacota o binário estático da release (scripts/package.sh rpm|srpm troca @VERSION@).
%global debug_package %{nil}

Name:           pcue
Version:        @VERSION@
Release:        1%{?dist}
Summary:        Rotinas para o desktop: abre apps, grava e reproduz ações
License:        LicenseRef-Proprietary
URL:            https://github.com/Parsec-Inc/pcue
Source0:        pcue-%{version}-linux-x86_64
Source1:        70-pcue.rules
Source2:        modules-load.conf
Source3:        postinst
ExclusiveArch:  x86_64
Requires(post): systemd-udev

%description
Rotinas para o desktop Linux: abre apps nos workspaces, grava e reproduz
ações, dispara por evento ou horário. Inclui a regra de udev que dá ao
usuário da sessão acesso ao teclado, mouse e uinput.

%install
install -Dm755 %{SOURCE0} %{buildroot}%{_bindir}/pcue
install -Dm644 %{SOURCE1} %{buildroot}%{_udevrulesdir}/70-pcue.rules
install -Dm644 %{SOURCE2} %{buildroot}%{_modulesloaddir}/pcue.conf
install -Dm755 %{SOURCE3} %{buildroot}%{_datadir}/pcue/postinst

%post
sh %{_datadir}/pcue/postinst

%files
%{_bindir}/pcue
%{_udevrulesdir}/70-pcue.rules
%{_modulesloaddir}/pcue.conf
%{_datadir}/pcue/postinst
