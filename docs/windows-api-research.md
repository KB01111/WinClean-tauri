# WinClean 2.0 — verified Windows API / platform research

Collected with the Microsoft Learn MCP server (`https://learn.microsoft.com/api/mcp`,
configured for development time in `.mcp.json` and `.vscode/mcp.json`) against the
official documentation. Each row says what the implementation relies on and
which parts are **preview / unverified** so the UI can say so.

## Service Control Manager

| Fact | Source | Used for |
|---|---|---|
| `ChangeServiceConfig` needs `SERVICE_CHANGE_CONFIG`, "should be granted only to administrators" | Service Security and Access Rights | Start-type writes only run in the elevated helper |
| Delayed auto-start = `ChangeServiceConfig2(SERVICE_CONFIG_DELAYED_AUTO_START_INFO)`; "takes effect after the next system boot" | Automatically Starting Services | Verification re-reads the flag but never claims a live effect |
| `EnumDependentServices` needs `SERVICE_ENUMERATE_DEPENDENTS`; result is reverse start order | EnumDependentServicesW | Dependents view + protection closure |
| Trigger-start services are configured with `SERVICE_CONFIG_TRIGGER_INFO` | Service Trigger Events | `triggerStart` flag: Manual + trigger is the idiomatic "don't run at boot" state |

## EcoQoS / process power throttling

* `SetProcessInformation(ProcessPowerThrottling)` — `ControlMask`+`StateMask` both
  `PROCESS_POWER_THROTTLING_EXECUTION_SPEED` → EcoQoS on; `ControlMask` set /
  `StateMask` 0 → forced HighQoS; **`ControlMask` 0 → system-managed default**.
  WinClean's restore always uses the last form so it never pins a process to HighQoS.
* "EcoQoS should not be used for performance critical or foreground user experiences."
* Windows ML efficiency tips: NPU background work should set
  `ep.dynamic.workload_type=Efficient` (only NPU EPs act on it); CPU background
  inference may use EcoQoS. Always verify with `GetProcessInformation`.

## Battery

* `IOCTL_BATTERY_QUERY_STATUS` → `BATTERY_STATUS { PowerState, Capacity (mWh), Voltage (mV), Rate (mW) }`.
  `Rate` negative = discharging, `BATTERY_UNKNOWN_RATE` (0x80000000) = unavailable,
  and **units are arbitrary if `BATTERY_CAPACITY_RELATIVE` is set** in
  `BATTERY_INFORMATION.Capabilities` — WinClean then reports no watts.
* `BATTERY_INFORMATION` → `DesignedCapacity`, `FullChargedCapacity`, `CycleCount` → health = full/design.
* All battery IOCTLs fail with `ERROR_NO_SUCH_DEVICE` (older: `ERROR_FILE_NOT_FOUND`) on a stale battery tag → re-query the tag.
* ACPI contract: _BST present rate must be accurate within `_BIX` measurement accuracy.
* `PowerSettingRegisterNotification(GUID, DEVICE_NOTIFY_CALLBACK, &DEVICE_NOTIFY_SUBSCRIBE_PARAMETERS)`
  needs no window, calls back immediately with the current value, and covers
  `GUID_ACDC_POWER_SOURCE` (PoAc/PoDc/PoHot) and `GUID_BATTERY_PERCENTAGE_REMAINING` → AC/DC and level
  changes are event-driven, not polled.

## Hybrid CPU topology

* `GetLogicalProcessorInformationEx(RelationAll)`; `PROCESSOR_RELATIONSHIP.EfficiencyClass`
  "is only nonzero on systems with a heterogeneous set of cores"; higher = more performance, less efficiency.
* `RelationCache` gives L3 size + the logical processors sharing it. WinClean derives
  clusters from EfficiencyClass **and** L3 sharing and only names them Zen 5 / Zen 5c when the
  CPU model is the HX 370 and the observed split is 4 + 8 — otherwise it reports "uniform".

## Windows ML / NPU

* Catalog EPs (MIGraphX, NvTensorRtRtx, OpenVINO, QNN, **VitisAI**, WebGPU-experimental) are
  downloaded on demand via `ExecutionProviderCatalog` and need **Windows 11 24H2 (build 26100)+**.
  Included: CPU, DirectML (legacy, maintenance mode).
* MIGraphX: AMD RDNA 3+ GPU, AMD driver ≥ 25.10.13.09 (Adrenalin version format — the registry
  `DriverVersion` uses a different format, so WinClean shows it but cannot compare), **not supported for GenAI**.
* NvTensorRtRtx: RTX 30xx+, driver ≥ 32.0.15.5585 (registry format, comparable), CUDA 12.5.
* VitisAI: AMD NPU; no numeric driver minimum is documented on the EP page, so none is invented.
* Windows ML is not bundled with WinClean (needs Windows App SDK). WinClean performs hardware, OS,
  driver, package and **static model** compatibility checks and ranks EPs; it does **not** run
  inference sessions. `session` verification therefore stays `notRun`.
* Copilot+ = 40+ TOPS NPU (registry cannot measure this → reported as unverified).

## MCP on Windows (preview)

* On-device Agent Registry (ODR), CLI `odr.exe`: `odr mcp list|run|add|remove|configure`,
  `odr agent-info list`. Requires **Windows build 26220.7262+**; "some information relates to
  pre-released product".
* Contained servers run in a separate agent session/user; user-file access needs explicit consent
  granted per host app. Unpackaged / `.mcpb` servers do not run contained unless the user enables
  "Reduce protections for agent connectors".
* File Explorer MCP connector tools: read-only — `read_text_file`, `search_files`, `get_file_details`,
  `read_file`, `get_directories`; destructive — `edit_text_file`, `create_directory`, `move_file`,
  `create_text_file`, `zip_folder`, `unzip_folder`. "File access requires explicit user permission."
  WinClean allow-lists the read-only set and blocks everything else, including unknown tools.
* `odr agent-info add/remove` cannot be used from unpackaged apps → WinClean never registers itself.
* Microsoft Learn MCP: hosted, no auth, `https://learn.microsoft.com/api/mcp` (HTTP transport).

## Things the previous baseline assumed that this research corrected

| Previous assumption | Correction |
|---|---|
| Agent Registry lives under `%LOCALAPPDATA%\Microsoft\Windows\Agents` | Not documented. Discovery is `odr.exe mcp list` |
| Windows ML EPs are DLLs in System32 | They are catalog-delivered packages; DLL probing is meaningless |
| Discharge watts from % deltas × design capacity | `BATTERY_STATUS.Rate` (mW) is the measurement; % deltas stay only as a cross-check |
