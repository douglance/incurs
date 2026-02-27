import { Cli, z } from '../src/index.js'

const cli = Cli.create('cloud', {
  description: 'Cloud infrastructure management CLI for benchmarking.',
  version: '0.1.0',
})

// ── compute ──────────────────────────────────────────────────────────────────

const compute = Cli.create('compute', { description: 'Manage compute instances and images' })

compute.command('instance-list', {
  description: 'List all compute instances',
  options: z.object({
    zone: z.string().optional().describe('Filter by availability zone'),
    status: z.string().optional().describe('Filter by status (running, stopped, terminated)'),
    limit: z.number().optional().describe('Maximum number of results'),
    format: z.string().optional().describe('Output format (table, json, csv)'),
  }),
  output: z.object({
    instances: z.array(z.object({
      name: z.string(),
      zone: z.string(),
      machineType: z.string(),
      status: z.string(),
      networkIP: z.string(),
      createdAt: z.string(),
    })),
    nextPageToken: z.string().optional(),
  }),
  run() { return { instances: [] } },
})

compute.command('instance-create', {
  description: 'Create a new virtual machine instance with the specified machine type and configuration. Requires a name and zone. Optionally configure disk type, networking, metadata, labels, and startup scripts. The instance starts automatically unless --no-start is specified.',
  args: z.object({ name: z.string().describe('Instance name') }),
  options: z.object({
    zone: z.string().describe('Availability zone where the instance will be created. Must be a valid zone in the target region (e.g. us-east1-b, europe-west1-c).'),
    machineType: z.string().optional().describe('Machine type that defines vCPUs and memory for the instance. Common types include e2-micro, e2-medium, n1-standard-1, n2-standard-4. Defaults to e2-medium if not specified.'),
    image: z.string().optional().describe('Boot disk image to use for the instance. Can be a full image path or a shorthand like debian-12 or ubuntu-2204-lts.'),
    diskSize: z.number().optional().describe('Boot disk size in GB. Must be at least 10 GB. Larger disks provide higher throughput and IOPS.'),
    diskType: z.enum(['pd-standard', 'pd-balanced', 'pd-ssd', 'pd-extreme']).default('pd-balanced').describe('Persistent disk type. pd-standard for cost-effective storage, pd-balanced for balanced performance, pd-ssd for high IOPS, pd-extreme for highest performance workloads.'),
    imageFamily: z.string().default('debian-12').describe('Image family to use when image is not specified. The latest non-deprecated image from this family will be selected automatically.'),
    network: z.string().optional().describe('VPC network name for the instance. Uses the default network if not specified. Must exist in the same project.'),
    subnet: z.string().optional().describe('Subnet name within the VPC network. Required when using custom-mode VPC networks.'),
    tags: z.array(z.string()).optional().describe('Network tags for firewall rule targeting. Tags are used to identify instances that firewall rules apply to.'),
    preemptible: z.boolean().optional().describe('Use a preemptible instance that costs significantly less but may be terminated at any time. Suitable for fault-tolerant batch workloads.'),
    serviceAccount: z.string().optional().describe('Service account email to attach to the instance. Controls which APIs the instance can access. Uses the default compute service account if not specified.'),
    noStart: z.boolean().optional().describe('Create the instance in a stopped state without starting it. Useful for configuring the instance before first boot.'),
    metadata: z.array(z.string()).optional().describe('Instance metadata in key=value format. Commonly used for startup-script, shutdown-script, and ssh-keys configuration.'),
    accelerator: z.string().optional().describe('GPU accelerator type to attach (e.g. nvidia-tesla-t4, nvidia-tesla-v100). Requires a compatible machine type like n1-standard or a2-highgpu.'),
    minCpuPlatform: z.string().optional().describe('Minimum CPU platform for the instance (e.g. "Intel Cascade Lake", "AMD Rome"). Ensures specific CPU features are available.'),
    labels: z.array(z.string()).optional().describe('Labels in key=value format for organizing and filtering instances. Used for cost allocation and resource management.'),
  }),
  alias: { zone: 'z', machineType: 't', preemptible: 'p' },
  env: z.object({
    CLOUD_PROJECT: z.string().describe('Active project ID for resource creation'),
    CLOUD_REGION: z.string().default('us-east1').describe('Default region when zone is not specified'),
  }),
  output: z.object({
    id: z.string().describe('Unique instance identifier'),
    name: z.string().describe('Instance name'),
    status: z.string().describe('Current instance status (PROVISIONING, STAGING, RUNNING, STOPPED)'),
    zone: z.string().describe('Zone where the instance was created'),
    machineType: z.string().describe('Machine type'),
    createdAt: z.string().describe('ISO 8601 creation timestamp'),
    disks: z.array(z.object({
      name: z.string(),
      sizeGb: z.number(),
      type: z.string(),
      boot: z.boolean(),
    })).describe('Attached disks'),
    networkInterfaces: z.array(z.object({
      network: z.string(),
      networkIP: z.string(),
      accessConfigs: z.array(z.object({
        type: z.string(),
        natIP: z.string().optional(),
      })),
    })).describe('Network interface configurations'),
  }),
  examples: [
    { args: { name: 'web-server-1' }, options: { zone: 'us-east1-b', machineType: 'e2-medium' }, description: 'Create a basic instance' },
    { args: { name: 'gpu-worker' }, options: { zone: 'us-central1-f', machineType: 'n1-standard-8', accelerator: 'nvidia-tesla-t4', diskSize: 200, diskType: 'pd-ssd' }, description: 'Create a GPU instance with SSD' },
    { args: { name: 'batch-job' }, options: { zone: 'us-west1-a', preemptible: true, noStart: true }, description: 'Create preemptible instance without starting' },
  ],
  run() {
    return {
      id: '1234567890', name: 'web-server-1', status: 'RUNNING', zone: 'us-east1-b',
      machineType: 'e2-medium', createdAt: '2025-01-01T00:00:00Z',
      disks: [{ name: 'boot', sizeGb: 10, type: 'pd-balanced', boot: true }],
      networkInterfaces: [{ network: 'default', networkIP: '10.0.0.2', accessConfigs: [{ type: 'ONE_TO_ONE_NAT' }] }],
    }
  },
})

compute.command('instance-delete', {
  description: 'Delete a compute instance',
  args: z.object({ name: z.string().describe('Instance name') }),
  options: z.object({
    zone: z.string().describe('Availability zone'),
    force: z.boolean().optional().describe('Skip confirmation'),
  }),
  run() { return { deleted: true } },
})

compute.command('instance-start', {
  description: 'Start a stopped compute instance',
  args: z.object({ name: z.string().describe('Instance name') }),
  options: z.object({ zone: z.string().describe('Availability zone') }),
  run() { return { started: true } },
})

compute.command('instance-stop', {
  description: 'Stop a running compute instance',
  args: z.object({ name: z.string().describe('Instance name') }),
  options: z.object({
    zone: z.string().describe('Availability zone'),
    discard: z.boolean().optional().describe('Discard local SSD data'),
  }),
  run() { return { stopped: true } },
})

compute.command('instance-ssh', {
  description: 'SSH into a compute instance',
  args: z.object({ name: z.string().describe('Instance name') }),
  options: z.object({
    zone: z.string().describe('Availability zone'),
    user: z.string().optional().describe('SSH user'),
    port: z.number().optional().describe('SSH port'),
    command: z.string().optional().describe('Remote command to execute'),
    identity: z.string().optional().describe('Path to SSH key'),
  }),
  run() { return { connected: true } },
})

compute.command('instance-resize', {
  description: 'Resize a compute instance machine type',
  args: z.object({ name: z.string().describe('Instance name') }),
  options: z.object({
    zone: z.string().describe('Availability zone'),
    machineType: z.string().describe('New machine type'),
  }),
  run() { return { resized: true } },
})

compute.command('image-list', {
  description: 'List available machine images',
  options: z.object({
    project: z.string().optional().describe('Filter by project'),
    family: z.string().optional().describe('Filter by image family'),
  }),
  run() { return { images: [] } },
})

compute.command('image-create', {
  description: 'Create a machine image from an instance disk',
  args: z.object({ name: z.string().describe('Image name') }),
  options: z.object({
    source: z.string().describe('Source disk or snapshot'),
    family: z.string().optional().describe('Image family'),
    description: z.string().optional().describe('Image description'),
  }),
  run() { return { created: true } },
})

compute.command('image-delete', {
  description: 'Delete a machine image',
  args: z.object({ name: z.string().describe('Image name') }),
  options: z.object({ force: z.boolean().optional().describe('Skip confirmation') }),
  run() { return { deleted: true } },
})

compute.command('snapshot-list', {
  description: 'List disk snapshots',
  options: z.object({ limit: z.number().optional().describe('Maximum results') }),
  run() { return { snapshots: [] } },
})

compute.command('snapshot-create', {
  description: 'Create a disk snapshot',
  args: z.object({ name: z.string().describe('Snapshot name') }),
  options: z.object({
    disk: z.string().describe('Source disk'),
    zone: z.string().describe('Disk zone'),
    description: z.string().optional().describe('Snapshot description'),
  }),
  run() { return { created: true } },
})

cli.command(compute)

// ── network ──────────────────────────────────────────────────────────────────

const network = Cli.create('network', { description: 'Manage VPC networks, subnets, and firewalls' })

network.command('vpc-list', {
  description: 'List VPC networks',
  options: z.object({ project: z.string().optional().describe('Filter by project') }),
  run() { return { vpcs: [] } },
})

network.command('vpc-create', {
  description: 'Create a VPC network',
  args: z.object({ name: z.string().describe('VPC name') }),
  options: z.object({
    mode: z.string().optional().describe('Subnet mode (auto or custom)'),
    mtu: z.number().optional().describe('Maximum transmission unit'),
    description: z.string().optional().describe('Network description'),
  }),
  run() { return { created: true } },
})

network.command('vpc-delete', {
  description: 'Delete a VPC network',
  args: z.object({ name: z.string().describe('VPC name') }),
  options: z.object({ force: z.boolean().optional().describe('Skip confirmation') }),
  run() { return { deleted: true } },
})

network.command('subnet-list', {
  description: 'List subnets in a VPC',
  options: z.object({
    vpc: z.string().optional().describe('Filter by VPC name'),
    region: z.string().optional().describe('Filter by region'),
  }),
  run() { return { subnets: [] } },
})

network.command('subnet-create', {
  description: 'Create a subnet',
  args: z.object({ name: z.string().describe('Subnet name') }),
  options: z.object({
    vpc: z.string().describe('VPC network name'),
    region: z.string().describe('Region'),
    cidr: z.string().describe('IP range in CIDR notation'),
    privateAccess: z.boolean().optional().describe('Enable private Google access'),
  }),
  run() { return { created: true } },
})

network.command('subnet-delete', {
  description: 'Delete a subnet',
  args: z.object({ name: z.string().describe('Subnet name') }),
  options: z.object({
    region: z.string().describe('Region'),
    force: z.boolean().optional().describe('Skip confirmation'),
  }),
  run() { return { deleted: true } },
})

network.command('firewall-list', {
  description: 'List firewall rules',
  options: z.object({ vpc: z.string().optional().describe('Filter by VPC') }),
  run() { return { rules: [] } },
})

network.command('firewall-create', {
  description: 'Create a firewall rule to allow or deny traffic to instances matching the specified target tags. Supports protocol and port specifications, source and destination IP ranges, and priority-based rule ordering.',
  args: z.object({ name: z.string().describe('Rule name') }),
  options: z.object({
    vpc: z.string().describe('VPC network'),
    allow: z.string().describe('Allowed protocols and ports (e.g. tcp:80,443)'),
    source: z.string().optional().describe('Source IP ranges'),
    target: z.string().optional().describe('Target tags'),
    priority: z.number().optional().describe('Rule priority (0-65535)'),
    direction: z.string().optional().describe('Traffic direction (ingress or egress)'),
    deny: z.string().optional().describe('Denied protocols and ports in the same format as --allow (e.g. tcp:25,udp:53)'),
    destinationRanges: z.string().optional().describe('Destination IP ranges in CIDR notation. Used with egress rules to restrict outbound traffic destinations.'),
    description: z.string().optional().describe('Human-readable description of the firewall rule purpose and intended traffic flow'),
    disabled: z.boolean().optional().describe('Create the rule in a disabled state. Disabled rules are not enforced but remain configured for later activation.'),
    logging: z.boolean().optional().describe('Enable firewall rule logging to capture connection records for matching traffic. Logs are sent to Cloud Logging.'),
  }),
  output: z.object({
    name: z.string(),
    network: z.string(),
    direction: z.string(),
    priority: z.number(),
    allowed: z.array(z.object({ protocol: z.string(), ports: z.array(z.string()) })),
    sourceRanges: z.array(z.string()),
    targetTags: z.array(z.string()),
    createdAt: z.string(),
  }),
  run() {
    return {
      name: 'allow-http', network: 'default', direction: 'INGRESS', priority: 1000,
      allowed: [{ protocol: 'tcp', ports: ['80', '443'] }],
      sourceRanges: ['0.0.0.0/0'], targetTags: ['http-server'], createdAt: '2025-01-01T00:00:00Z',
    }
  },
})

network.command('firewall-delete', {
  description: 'Delete a firewall rule',
  args: z.object({ name: z.string().describe('Rule name') }),
  options: z.object({ force: z.boolean().optional().describe('Skip confirmation') }),
  run() { return { deleted: true } },
})

network.command('dns-list', {
  description: 'List DNS zones',
  options: z.object({ project: z.string().optional().describe('Filter by project') }),
  run() { return { zones: [] } },
})

network.command('dns-create', {
  description: 'Create a DNS zone',
  args: z.object({ name: z.string().describe('Zone name') }),
  options: z.object({
    domain: z.string().describe('DNS domain name'),
    description: z.string().optional().describe('Zone description'),
    visibility: z.string().optional().describe('Zone visibility (public or private)'),
  }),
  run() { return { created: true } },
})

network.command('lb-list', {
  description: 'List load balancers',
  options: z.object({ region: z.string().optional().describe('Filter by region') }),
  run() { return { loadBalancers: [] } },
})

cli.command(network)

// ── storage ──────────────────────────────────────────────────────────────────

const storage = Cli.create('storage', { description: 'Manage object storage buckets and files' })

storage.command('bucket-list', {
  description: 'List storage buckets',
  options: z.object({
    project: z.string().optional().describe('Filter by project'),
    prefix: z.string().optional().describe('Filter by bucket name prefix'),
  }),
  run() { return { buckets: [] } },
})

storage.command('bucket-create', {
  description: 'Create a new storage bucket with the specified storage class and region. Supports versioning, lifecycle policies, CORS configuration, and access control settings.',
  args: z.object({ name: z.string().describe('Bucket name') }),
  options: z.object({
    region: z.string().optional().describe('Bucket region'),
    storageClass: z.string().optional().describe('Storage class (standard, nearline, coldline, archive)'),
    versioning: z.boolean().optional().describe('Enable object versioning'),
    cors: z.boolean().optional().describe('Enable CORS'),
    retention: z.number().optional().describe('Retention period in days for compliance. Objects cannot be deleted or overwritten until the retention period expires.'),
    uniformAccess: z.boolean().optional().describe('Enable uniform bucket-level access. When enabled, ACLs are disabled and all access is controlled by IAM policies.'),
    labels: z.array(z.string()).optional().describe('Key=value labels for organization and billing. Used to categorize buckets across projects and teams.'),
    publicAccess: z.enum(['allow', 'deny']).default('deny').describe('Public access prevention setting. Set to deny to enforce that no objects in the bucket can be made publicly accessible.'),
  }),
  alias: { region: 'r', storageClass: 's' },
  env: z.object({
    CLOUD_PROJECT: z.string().describe('Active project ID'),
    CLOUD_STORAGE_DEFAULT_CLASS: z.string().default('standard').describe('Default storage class for new buckets'),
  }),
  output: z.object({
    name: z.string(),
    region: z.string(),
    storageClass: z.string(),
    createdAt: z.string(),
    versioning: z.boolean(),
    cors: z.array(z.object({ origin: z.string(), method: z.string(), maxAge: z.number() })).optional(),
    lifecycle: z.object({ rules: z.array(z.object({ action: z.string(), condition: z.object({ age: z.number().optional() }) })) }).optional(),
  }),
  examples: [
    { args: { name: 'my-data-lake' }, options: { region: 'us-east1', storageClass: 'standard', versioning: true }, description: 'Create a versioned bucket' },
    { args: { name: 'archive-2024' }, options: { region: 'eu-west1', storageClass: 'coldline', retention: 365, uniformAccess: true }, description: 'Create a compliant archive bucket' },
  ],
  run() {
    return {
      name: 'my-data-lake', region: 'us-east1', storageClass: 'standard',
      createdAt: '2025-01-01T00:00:00Z', versioning: true,
    }
  },
})

storage.command('bucket-delete', {
  description: 'Delete a storage bucket',
  args: z.object({ name: z.string().describe('Bucket name') }),
  options: z.object({
    force: z.boolean().optional().describe('Delete even if bucket is not empty'),
    recursive: z.boolean().optional().describe('Delete all objects first'),
  }),
  run() { return { deleted: true } },
})

storage.command('object-list', {
  description: 'List objects in a bucket',
  args: z.object({ bucket: z.string().describe('Bucket name') }),
  options: z.object({
    prefix: z.string().optional().describe('Object key prefix'),
    delimiter: z.string().optional().describe('Delimiter for hierarchy'),
    limit: z.number().optional().describe('Maximum results'),
    recursive: z.boolean().optional().describe('List all objects recursively'),
  }),
  output: z.object({
    objects: z.array(z.object({
      key: z.string(),
      size: z.number().describe('Size in bytes'),
      lastModified: z.string(),
      storageClass: z.string(),
      etag: z.string(),
    })),
    prefixes: z.array(z.string()).optional().describe('Common prefixes when using delimiter'),
    nextPageToken: z.string().optional(),
  }),
  run() { return { objects: [] } },
})

storage.command('object-upload', {
  description: 'Upload a file to a bucket',
  args: z.object({
    source: z.string().describe('Local file path'),
    destination: z.string().describe('Bucket path (bucket/key)'),
  }),
  options: z.object({
    contentType: z.string().optional().describe('Content type override'),
    cacheControl: z.string().optional().describe('Cache-Control header'),
    acl: z.string().optional().describe('Object ACL (private, public-read)'),
  }),
  run() { return { uploaded: true } },
})

storage.command('object-download', {
  description: 'Download a file from a bucket',
  args: z.object({
    source: z.string().describe('Bucket path (bucket/key)'),
    destination: z.string().describe('Local file path'),
  }),
  run() { return { downloaded: true } },
})

storage.command('object-delete', {
  description: 'Delete an object from a bucket',
  args: z.object({ path: z.string().describe('Object path (bucket/key)') }),
  options: z.object({
    recursive: z.boolean().optional().describe('Delete all objects matching prefix'),
    force: z.boolean().optional().describe('Skip confirmation'),
  }),
  run() { return { deleted: true } },
})

storage.command('object-copy', {
  description: 'Copy an object between buckets or paths',
  args: z.object({
    source: z.string().describe('Source path (bucket/key)'),
    destination: z.string().describe('Destination path (bucket/key)'),
  }),
  options: z.object({
    recursive: z.boolean().optional().describe('Copy all objects matching prefix'),
  }),
  run() { return { copied: true } },
})

storage.command('presign', {
  description: 'Generate a presigned URL for an object',
  args: z.object({ path: z.string().describe('Object path (bucket/key)') }),
  options: z.object({
    expires: z.number().optional().describe('URL expiration in seconds'),
    method: z.string().optional().describe('HTTP method (GET, PUT)'),
  }),
  run() { return { url: 'https://example.com/presigned' } },
})

storage.command('sync', {
  description: 'Sync a local directory to a bucket or vice versa',
  args: z.object({
    source: z.string().describe('Source path'),
    destination: z.string().describe('Destination path'),
  }),
  options: z.object({
    delete: z.boolean().optional().describe('Delete files in destination not in source'),
    dryRun: z.boolean().optional().describe('Show what would be synced'),
    exclude: z.string().optional().describe('Exclude pattern'),
  }),
  run() { return { synced: true } },
})

storage.command('lifecycle-set', {
  description: 'Set lifecycle rules on a bucket',
  args: z.object({ bucket: z.string().describe('Bucket name') }),
  options: z.object({
    expireDays: z.number().optional().describe('Delete objects after N days'),
    transitionDays: z.number().optional().describe('Transition storage class after N days'),
    transitionClass: z.string().optional().describe('Target storage class'),
  }),
  run() { return { set: true } },
})

storage.command('cors-set', {
  description: 'Set CORS configuration on a bucket',
  args: z.object({ bucket: z.string().describe('Bucket name') }),
  options: z.object({
    origins: z.array(z.string()).describe('Allowed origins'),
    methods: z.array(z.string()).optional().describe('Allowed HTTP methods'),
    maxAge: z.number().optional().describe('Max age in seconds'),
  }),
  run() { return { set: true } },
})

cli.command(storage)

// ── database ─────────────────────────────────────────────────────────────────

const database = Cli.create('database', { description: 'Manage database instances and backups' })

database.command('instance-list', {
  description: 'List database instances',
  options: z.object({
    engine: z.string().optional().describe('Filter by engine (postgres, mysql, redis)'),
    region: z.string().optional().describe('Filter by region'),
    status: z.string().optional().describe('Filter by status'),
  }),
  output: z.object({
    instances: z.array(z.object({
      name: z.string(),
      engine: z.string(),
      version: z.string(),
      tier: z.string(),
      region: z.string(),
      status: z.string(),
      connectionName: z.string(),
    })),
  }),
  run() { return { instances: [] } },
})

database.command('instance-create', {
  description: 'Create a managed database instance with the specified engine, version, and configuration. Supports PostgreSQL, MySQL, and Redis. Configure high availability, automated backups, maintenance windows, and network settings.',
  args: z.object({ name: z.string().describe('Instance name') }),
  options: z.object({
    engine: z.string().describe('Database engine (postgres, mysql, redis)'),
    version: z.string().optional().describe('Engine version'),
    tier: z.string().optional().describe('Machine tier'),
    region: z.string().describe('Region'),
    storage: z.number().optional().describe('Storage size in GB'),
    ha: z.boolean().optional().describe('Enable high availability'),
    password: z.string().optional().describe('Root password'),
    backupEnabled: z.boolean().default(true).describe('Enable automated daily backups. Backups are retained for 7 days by default.'),
    backupWindow: z.string().optional().describe('Preferred backup window in UTC (e.g. 03:00-04:00). If not specified, the system chooses an optimal window.'),
    maintenanceWindow: z.string().optional().describe('Preferred maintenance day and hour in UTC (e.g. Mon:03:00). Updates and patches are applied during this window.'),
    network: z.string().optional().describe('VPC network for private IP connectivity. When specified, the instance is accessible only from within the VPC.'),
    ipWhitelist: z.array(z.string()).optional().describe('Allowed IP ranges for database connections in CIDR notation. Required when using public IP connectivity.'),
    flags: z.array(z.string()).optional().describe('Database engine flags in key=value format (e.g. max_connections=200, log_min_duration_statement=1000).'),
    labels: z.array(z.string()).optional().describe('Key=value labels for organizing instances and tracking costs across teams and environments.'),
  }),
  alias: { engine: 'e', region: 'r', tier: 't' },
  env: z.object({
    CLOUD_PROJECT: z.string().describe('Active project ID'),
    CLOUD_DB_DEFAULT_ENGINE: z.string().default('postgres').describe('Default database engine'),
  }),
  output: z.object({
    name: z.string(),
    engine: z.string(),
    version: z.string(),
    tier: z.string(),
    region: z.string(),
    status: z.string().describe('Instance status (CREATING, RUNNING, STOPPED, FAILED)'),
    connectionName: z.string().describe('Connection string identifier'),
    ipAddresses: z.array(z.object({ type: z.string(), address: z.string() })),
    settings: z.object({
      backupEnabled: z.boolean(),
      backupWindow: z.string().optional(),
      maintenanceWindow: z.string().optional(),
      storageGb: z.number(),
      ha: z.boolean(),
    }),
    createdAt: z.string(),
  }),
  examples: [
    { args: { name: 'users-db' }, options: { engine: 'postgres', region: 'us-east1', tier: 'db-standard-2' }, description: 'Create a PostgreSQL instance' },
    { args: { name: 'cache-prod' }, options: { engine: 'redis', region: 'us-central1', tier: 'db-highmem-1', ha: true }, description: 'Create a Redis instance with HA' },
    { args: { name: 'analytics-db' }, options: { engine: 'mysql', region: 'eu-west1', storage: 500, backupWindow: '02:00-03:00', ipWhitelist: ['10.0.0.0/8'] }, description: 'Create MySQL with backup window and IP whitelist' },
  ],
  run() {
    return {
      name: 'users-db', engine: 'postgres', version: '15', tier: 'db-standard-2',
      region: 'us-east1', status: 'RUNNING', connectionName: 'proj:us-east1:users-db',
      ipAddresses: [{ type: 'PRIMARY', address: '10.0.0.5' }],
      settings: { backupEnabled: true, storageGb: 100, ha: false },
      createdAt: '2025-01-01T00:00:00Z',
    }
  },
})

database.command('instance-delete', {
  description: 'Delete a database instance',
  args: z.object({ name: z.string().describe('Instance name') }),
  options: z.object({
    force: z.boolean().optional().describe('Skip confirmation'),
    keepBackups: z.boolean().optional().describe('Retain automated backups'),
  }),
  run() { return { deleted: true } },
})

database.command('instance-restart', {
  description: 'Restart a database instance',
  args: z.object({ name: z.string().describe('Instance name') }),
  run() { return { restarted: true } },
})

database.command('instance-resize', {
  description: 'Resize a database instance tier',
  args: z.object({ name: z.string().describe('Instance name') }),
  options: z.object({
    tier: z.string().describe('New machine tier'),
    storage: z.number().optional().describe('New storage size in GB'),
  }),
  run() { return { resized: true } },
})

database.command('instance-connect', {
  description: 'Connect to a database instance via proxy',
  args: z.object({ name: z.string().describe('Instance name') }),
  options: z.object({
    port: z.number().optional().describe('Local proxy port'),
    user: z.string().optional().describe('Database user'),
    database: z.string().optional().describe('Database name'),
  }),
  run() { return { connected: true } },
})

database.command('backup-list', {
  description: 'List backups for a database instance',
  args: z.object({ instance: z.string().describe('Instance name') }),
  options: z.object({ limit: z.number().optional().describe('Maximum results') }),
  run() { return { backups: [] } },
})

database.command('backup-create', {
  description: 'Create a manual backup',
  args: z.object({ instance: z.string().describe('Instance name') }),
  options: z.object({ description: z.string().optional().describe('Backup description') }),
  run() { return { created: true } },
})

database.command('backup-restore', {
  description: 'Restore a database from backup',
  args: z.object({ backup: z.string().describe('Backup ID') }),
  options: z.object({
    target: z.string().optional().describe('Target instance name (creates new if omitted)'),
    pointInTime: z.string().optional().describe('Point-in-time recovery timestamp'),
  }),
  run() { return { restored: true } },
})

database.command('replica-create', {
  description: 'Create a read replica',
  args: z.object({ name: z.string().describe('Replica name') }),
  options: z.object({
    source: z.string().describe('Source instance name'),
    region: z.string().optional().describe('Replica region'),
    tier: z.string().optional().describe('Replica machine tier'),
  }),
  run() { return { created: true } },
})

database.command('user-list', {
  description: 'List database users',
  args: z.object({ instance: z.string().describe('Instance name') }),
  run() { return { users: [] } },
})

database.command('user-create', {
  description: 'Create a database user',
  args: z.object({
    instance: z.string().describe('Instance name'),
    username: z.string().describe('Username'),
  }),
  options: z.object({
    password: z.string().optional().describe('User password'),
    role: z.string().optional().describe('User role'),
  }),
  run() { return { created: true } },
})

cli.command(database)

// ── deploy ───────────────────────────────────────────────────────────────────

const deploy = Cli.create('deploy', { description: 'Manage deployments, rollouts, and services' })

deploy.command('service-list', {
  description: 'List deployed services',
  options: z.object({
    environment: z.string().optional().describe('Filter by environment (production, staging, dev)'),
    region: z.string().optional().describe('Filter by region'),
    status: z.string().optional().describe('Filter by status'),
  }),
  run() { return { services: [] } },
})

deploy.command('service-create', {
  description: 'Deploy a new service from a container image. Configure replicas, resource limits, environment variables, health checks, and autoscaling policies. Services are deployed to the specified region with automatic load balancing.',
  args: z.object({ name: z.string().describe('Service name') }),
  options: z.object({
    image: z.string().describe('Container image'),
    port: z.number().optional().describe('Container port'),
    replicas: z.number().optional().describe('Number of replicas'),
    memory: z.string().optional().describe('Memory limit (e.g. 512Mi, 1Gi)'),
    cpu: z.string().optional().describe('CPU limit (e.g. 500m, 1)'),
    env: z.array(z.string()).optional().describe('Environment variables (KEY=VALUE)'),
    region: z.string().optional().describe('Deployment region'),
    minScale: z.number().optional().describe('Minimum instances'),
    maxScale: z.number().optional().describe('Maximum instances'),
    healthCheckPath: z.string().optional().describe('HTTP path for liveness and readiness health checks. The service must return 200 OK at this path.'),
    healthCheckInterval: z.number().optional().describe('Health check interval in seconds. Determines how frequently the load balancer checks instance health.'),
    timeout: z.number().optional().describe('Request timeout in seconds. Requests exceeding this duration are terminated with a 504 status.'),
    concurrency: z.number().optional().describe('Maximum concurrent requests per instance. When exceeded, new requests are routed to other instances or queued.'),
    serviceAccount: z.string().optional().describe('Service account email for the service. Controls which cloud APIs the service can access at runtime.'),
    labels: z.array(z.string()).optional().describe('Labels for the service in key=value format. Used for filtering, cost tracking, and organizational purposes.'),
    noTraffic: z.boolean().optional().describe('Deploy the new revision without routing traffic to it. Useful for testing a deployment before switching traffic.'),
  }),
  alias: { port: 'p', replicas: 'r', image: 'i' },
  env: z.object({
    CLOUD_PROJECT: z.string().describe('Active project ID'),
    CLOUD_DEPLOY_REGION: z.string().default('us-central1').describe('Default deployment region'),
  }),
  output: z.object({
    name: z.string(),
    url: z.string().describe('Service URL'),
    revision: z.string().describe('Deployed revision name'),
    status: z.string(),
    image: z.string(),
    replicas: z.object({ desired: z.number(), ready: z.number() }),
    resources: z.object({ cpu: z.string(), memory: z.string() }),
    createdAt: z.string(),
  }),
  examples: [
    { args: { name: 'api' }, options: { image: 'myapp:latest', port: 8080, healthCheckPath: '/healthz', replicas: 3 }, description: 'Deploy a service with health checks' },
    { args: { name: 'worker' }, options: { image: 'worker:v2', cpu: '2', memory: '4Gi', minScale: 1, maxScale: 10, concurrency: 50 }, description: 'Deploy an autoscaling worker service' },
  ],
  run() {
    return {
      name: 'api', url: 'https://api-xyz.run.app', revision: 'api-00001',
      status: 'ACTIVE', image: 'myapp:latest',
      replicas: { desired: 3, ready: 3 }, resources: { cpu: '1', memory: '512Mi' },
      createdAt: '2025-01-01T00:00:00Z',
    }
  },
})

deploy.command('service-delete', {
  description: 'Delete a deployed service',
  args: z.object({ name: z.string().describe('Service name') }),
  options: z.object({ force: z.boolean().optional().describe('Skip confirmation') }),
  run() { return { deleted: true } },
})

deploy.command('service-update', {
  description: 'Update a deployed service',
  args: z.object({ name: z.string().describe('Service name') }),
  options: z.object({
    image: z.string().optional().describe('New container image'),
    replicas: z.number().optional().describe('New replica count'),
    memory: z.string().optional().describe('New memory limit'),
    cpu: z.string().optional().describe('New CPU limit'),
    env: z.array(z.string()).optional().describe('Updated environment variables'),
  }),
  run() { return { updated: true } },
})

deploy.command('service-logs', {
  description: 'Stream logs from a deployed service',
  args: z.object({ name: z.string().describe('Service name') }),
  options: z.object({
    follow: z.boolean().optional().describe('Follow log output'),
    tail: z.number().optional().describe('Number of recent lines'),
    since: z.string().optional().describe('Show logs since timestamp'),
    container: z.string().optional().describe('Specific container name'),
  }),
  run() { return { logs: [] } },
})

deploy.command('rollout-list', {
  description: 'List rollout history for a service',
  args: z.object({ service: z.string().describe('Service name') }),
  options: z.object({ limit: z.number().optional().describe('Maximum results') }),
  run() { return { rollouts: [] } },
})

deploy.command('rollout-status', {
  description: 'Check status of a rollout',
  args: z.object({ service: z.string().describe('Service name') }),
  run() { return { status: 'running' } },
})

deploy.command('rollout-rollback', {
  description: 'Rollback a service to a previous revision',
  args: z.object({ service: z.string().describe('Service name') }),
  options: z.object({
    revision: z.number().optional().describe('Target revision number'),
  }),
  run() { return { rolledBack: true } },
})

deploy.command('secret-list', {
  description: 'List secrets available to services',
  options: z.object({
    environment: z.string().optional().describe('Filter by environment'),
  }),
  run() { return { secrets: [] } },
})

deploy.command('secret-set', {
  description: 'Set a secret value',
  args: z.object({
    name: z.string().describe('Secret name'),
    value: z.string().describe('Secret value'),
  }),
  options: z.object({
    environment: z.string().optional().describe('Target environment'),
  }),
  run() { return { set: true } },
})

deploy.command('secret-delete', {
  description: 'Delete a secret',
  args: z.object({ name: z.string().describe('Secret name') }),
  options: z.object({
    environment: z.string().optional().describe('Target environment'),
    force: z.boolean().optional().describe('Skip confirmation'),
  }),
  run() { return { deleted: true } },
})

deploy.command('domain-list', {
  description: 'List custom domains',
  run() { return { domains: [] } },
})

cli.command(deploy)

// ── iam ──────────────────────────────────────────────────────────────────────

const iam = Cli.create('iam', { description: 'Manage identity, roles, and access policies' })

iam.command('user-list', {
  description: 'List IAM users',
  options: z.object({
    role: z.string().optional().describe('Filter by role'),
    status: z.string().optional().describe('Filter by status (active, suspended)'),
  }),
  run() { return { users: [] } },
})

iam.command('user-create', {
  description: 'Create an IAM user',
  args: z.object({ email: z.string().describe('User email address') }),
  options: z.object({
    role: z.string().optional().describe('Initial role assignment'),
    displayName: z.string().optional().describe('Display name'),
    groups: z.array(z.string()).optional().describe('Group memberships'),
  }),
  run() { return { created: true } },
})

iam.command('user-delete', {
  description: 'Delete an IAM user',
  args: z.object({ email: z.string().describe('User email') }),
  options: z.object({ force: z.boolean().optional().describe('Skip confirmation') }),
  run() { return { deleted: true } },
})

iam.command('role-list', {
  description: 'List available IAM roles',
  options: z.object({ custom: z.boolean().optional().describe('Show only custom roles') }),
  run() { return { roles: [] } },
})

iam.command('role-create', {
  description: 'Create a custom IAM role with the specified permissions. Roles can be scoped to projects or organizations. Supports condition expressions for fine-grained access control.',
  args: z.object({ name: z.string().describe('Role name') }),
  options: z.object({
    permissions: z.array(z.string()).describe('List of permissions'),
    description: z.string().optional().describe('Role description'),
    title: z.string().optional().describe('Human-readable role title displayed in the console and API responses. Defaults to the role name if not specified.'),
    stage: z.enum(['alpha', 'beta', 'ga', 'deprecated']).default('ga').describe('Role launch stage. Alpha and beta roles may change without notice. Deprecated roles should not be used for new bindings.'),
    condition: z.string().optional().describe('CEL condition expression for conditional role bindings. Allows restricting when the role applies based on resource attributes, time, or request context.'),
  }),
  alias: { permissions: 'p' },
  output: z.object({
    name: z.string(),
    title: z.string().optional(),
    permissions: z.array(z.string()),
    stage: z.string(),
    includedPermissions: z.number().describe('Total permissions granted'),
    createdAt: z.string(),
  }),
  run() {
    return {
      name: 'customEditor', permissions: ['storage.objects.get', 'storage.objects.list'],
      stage: 'ga', includedPermissions: 2, createdAt: '2025-01-01T00:00:00Z',
    }
  },
})

iam.command('role-delete', {
  description: 'Delete a custom IAM role',
  args: z.object({ name: z.string().describe('Role name') }),
  options: z.object({ force: z.boolean().optional().describe('Skip confirmation') }),
  run() { return { deleted: true } },
})

iam.command('policy-get', {
  description: 'Get IAM policy for a resource',
  args: z.object({ resource: z.string().describe('Resource ID') }),
  run() { return { policy: {} } },
})

iam.command('policy-set', {
  description: 'Set IAM policy for a resource',
  args: z.object({ resource: z.string().describe('Resource ID') }),
  options: z.object({
    member: z.string().describe('Member (user:email or group:name)'),
    role: z.string().describe('Role to grant'),
  }),
  run() { return { set: true } },
})

iam.command('policy-remove', {
  description: 'Remove a member from a resource policy',
  args: z.object({ resource: z.string().describe('Resource ID') }),
  options: z.object({
    member: z.string().describe('Member to remove'),
    role: z.string().describe('Role to revoke'),
  }),
  run() { return { removed: true } },
})

iam.command('token-create', {
  description: 'Create a service account token',
  args: z.object({ account: z.string().describe('Service account email') }),
  options: z.object({
    scopes: z.array(z.string()).optional().describe('Token scopes'),
    lifetime: z.string().optional().describe('Token lifetime (e.g. 1h, 24h)'),
  }),
  run() { return { token: 'tok_...' } },
})

iam.command('token-revoke', {
  description: 'Revoke a service account token',
  args: z.object({ token: z.string().describe('Token ID') }),
  run() { return { revoked: true } },
})

iam.command('audit-log', {
  description: 'View IAM audit logs',
  options: z.object({
    since: z.string().optional().describe('Show logs since timestamp'),
    user: z.string().optional().describe('Filter by user'),
    action: z.string().optional().describe('Filter by action type'),
    limit: z.number().optional().describe('Maximum results'),
  }),
  run() { return { logs: [] } },
})

cli.command(iam)

export default cli
