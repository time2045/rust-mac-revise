$ErrorActionPreference = 'Stop'
[Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false)
$rows = @(Get-NetAdapter -IncludeHidden -ErrorAction Stop | ForEach-Object {
    [PSCustomObject]@{
        guid = ([guid]$_.InterfaceGuid).ToString('D').ToUpperInvariant()
        name = [string]$_.Name
        description = [string]$_.InterfaceDescription
        pnp_id = [string]$_.PnPDeviceID
        mac = [string]$_.MacAddress
        permanent_mac = [string]$_.PermanentAddress
        admin_enabled = ([int]$_.InterfaceAdminStatus -eq 1)
        status = [string]$_.Status
        hardware = [bool]$_.HardwareInterface
    }
})
ConvertTo-Json -InputObject $rows -Depth 3 -Compress
