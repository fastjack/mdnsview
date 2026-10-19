# Spec Delta

## ADDED Requirements

### Requirement: Selection follows merges
When the selected device is merged with another device or split into separate devices, the selection SHALL move to the device that contains the previously selected device's host name, instead of jumping to another entry. A retained entry SHALL stay retained when it becomes part of a merged device.

#### Scenario: Selected device is merged
- **WHEN** the user has selected the device `1e8da75b.local` and it is merged into the device "Hyperion"
- **THEN** the merged "Hyperion" device is selected

#### Scenario: Selected device splits
- **WHEN** the user has selected a merged device and its host names stop sharing an address
- **THEN** the device holding the host name the merged device was identified by stays selected

### Requirement: Device host names in details
The details pane SHALL make all host names of a device visible:
- the simple service view SHALL include a "Device" line listing every host name of the service's device when the device has more than one
- when a device is selected and none of its services is, the device details SHALL list every host name together with the device's addresses, number of services and state

The filter SHALL match any of a device's host names.

#### Scenario: Merged device in the service view
- **WHEN** a service of the merged device "Hyperion" is selected
- **THEN** the simple details view shows a "Device" line listing `Hyperion.local` and `1e8da75b.local`

#### Scenario: Single host device
- **WHEN** a service of a device with one host name is selected
- **THEN** no "Device" line is shown

#### Scenario: Filter by secondary host name
- **WHEN** the user types `/1e8da75b` in device mode
- **THEN** the merged device "Hyperion" remains listed
