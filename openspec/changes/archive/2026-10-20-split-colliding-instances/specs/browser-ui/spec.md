# Spec Delta

## ADDED Requirements

### Requirement: Shared instance name in details
When the selected service's instance name is also announced by other hosts, the simple details view SHALL show a "Shared name" line listing those other host names.

#### Scenario: TeamViewer instance shared by two PCs
- **WHEN** the `0._teamviewer._tcp` service of `DESKTOP-EAOAATI.local` is selected and `DESKTOP-28SEPKJ-2.local` announces the same instance name
- **THEN** the simple details view shows a "Shared name" line listing `DESKTOP-28SEPKJ-2.local`

#### Scenario: Instance name not shared
- **WHEN** a service whose instance name is announced by only one host is selected
- **THEN** no "Shared name" line is shown

### Requirement: Selection follows a shared instance's host
When a second host starts or stops announcing the selected service's instance name, the selection SHALL stay on the service of the same instance name and host.

#### Scenario: Twin appears
- **WHEN** the `0._teamviewer._tcp` service of `DESKTOP-EAOAATI.local` is selected and `DESKTOP-28SEPKJ-2.local` starts announcing the same instance name
- **THEN** the `DESKTOP-EAOAATI.local` service remains selected

#### Scenario: Twin removed
- **WHEN** the `0._teamviewer._tcp` service of `DESKTOP-28SEPKJ-2.local` is selected and the `DESKTOP-EAOAATI.local` service of that instance name is removed
- **THEN** the `DESKTOP-28SEPKJ-2.local` service remains selected
