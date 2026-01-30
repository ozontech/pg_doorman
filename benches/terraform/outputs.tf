# ============================================
# AWS Account Info
# ============================================

output "aws_account_id" {
  description = "AWS Account ID"
  value       = data.aws_caller_identity.current.account_id
}

output "aws_region" {
  description = "AWS Region"
  value       = var.aws_region
}

# ============================================
# GitHub Actions Credentials
# ============================================

output "github_actions_access_key_id" {
  description = "Access Key ID for GitHub Actions (AWS_ACCESS_KEY_ID)"
  value       = aws_iam_access_key.github_actions.id
  sensitive   = false
}

output "github_actions_secret_access_key" {
  description = "Secret Access Key for GitHub Actions (AWS_SECRET_ACCESS_KEY)"
  value       = aws_iam_access_key.github_actions.secret
  sensitive   = true
}

# ============================================
# ECR Repository
# ============================================

output "ecr_repository_url" {
  description = "ECR repository URL (ECR_REPOSITORY)"
  value       = aws_ecr_repository.pg_doorman_bench.repository_url
}

output "ecr_repository_arn" {
  description = "ECR repository ARN"
  value       = aws_ecr_repository.pg_doorman_bench.arn
}

output "ecr_repository_name" {
  description = "ECR repository name"
  value       = aws_ecr_repository.pg_doorman_bench.name
}

# ============================================
# ECS/Fargate Configuration
# ============================================

output "ecs_cluster_name" {
  description = "ECS cluster name (ECS_CLUSTER_NAME)"
  value       = aws_ecs_cluster.bench_cluster.name
}

output "ecs_cluster_arn" {
  description = "ECS cluster ARN"
  value       = aws_ecs_cluster.bench_cluster.arn
}

output "ecs_task_execution_role_arn" {
  description = "ECS task execution role ARN (ECS_TASK_EXECUTION_ROLE_ARN)"
  value       = aws_iam_role.ecs_task_execution_role.arn
}

output "ecs_task_role_arn" {
  description = "ECS task role ARN (ECS_TASK_ROLE_ARN)"
  value       = aws_iam_role.ecs_task_role.arn
}

output "ecs_security_group_id" {
  description = "Security group ID for Fargate tasks (ECS_SECURITY_GROUP_ID)"
  value       = aws_security_group.fargate_sg.id
}

output "ecs_subnet_ids" {
  description = "Subnet IDs for Fargate tasks (ECS_SUBNET_IDS)"
  value       = data.aws_subnets.default.ids
}

output "ecs_log_group_name" {
  description = "CloudWatch log group name for ECS"
  value       = aws_cloudwatch_log_group.ecs_logs.name
}

# ============================================
# GitHub Secrets Summary
# ============================================

output "github_secrets_summary" {
  description = "All required values for GitHub Secrets"
  value = {
    AWS_ACCESS_KEY_ID            = aws_iam_access_key.github_actions.id
    AWS_SECRET_ACCESS_KEY        = aws_iam_access_key.github_actions.secret
    AWS_REGION                   = var.aws_region
    ECR_REPOSITORY               = aws_ecr_repository.pg_doorman_bench.repository_url
    ECS_CLUSTER_NAME             = aws_ecs_cluster.bench_cluster.name
    ECS_TASK_EXECUTION_ROLE_ARN  = aws_iam_role.ecs_task_execution_role.arn
    ECS_TASK_ROLE_ARN            = aws_iam_role.ecs_task_role.arn
    ECS_SECURITY_GROUP_ID        = aws_security_group.fargate_sg.id
    ECS_SUBNET_IDS               = join(",", data.aws_subnets.default.ids)
  }
  sensitive = true
}

# ============================================
# Setup Commands
# ============================================

output "setup_commands" {
  description = "Commands to set up GitHub Secrets via gh CLI"
  value = <<-EOT
    # Export environment variables:
    export AWS_ACCESS_KEY_ID="${aws_iam_access_key.github_actions.id}"
    export AWS_SECRET_ACCESS_KEY="${nonsensitive(aws_iam_access_key.github_actions.secret)}"
    export AWS_REGION="${var.aws_region}"
    export ECR_REPOSITORY="${aws_ecr_repository.pg_doorman_bench.repository_url}"
    export ECS_CLUSTER_NAME="${aws_ecs_cluster.bench_cluster.name}"
    export ECS_TASK_EXECUTION_ROLE_ARN="${aws_iam_role.ecs_task_execution_role.arn}"
    export ECS_TASK_ROLE_ARN="${aws_iam_role.ecs_task_role.arn}"
    export ECS_SECURITY_GROUP_ID="${aws_security_group.fargate_sg.id}"
    export ECS_SUBNET_IDS="${join(",", data.aws_subnets.default.ids)}"

    # Add to GitHub Secrets (requires gh CLI):
    gh secret set AWS_ACCESS_KEY_ID -b"${aws_iam_access_key.github_actions.id}"
    gh secret set AWS_SECRET_ACCESS_KEY -b"${nonsensitive(aws_iam_access_key.github_actions.secret)}"
    gh secret set AWS_REGION -b"${var.aws_region}"
    gh secret set ECR_REPOSITORY -b"${aws_ecr_repository.pg_doorman_bench.repository_url}"
    gh secret set ECS_CLUSTER_NAME -b"${aws_ecs_cluster.bench_cluster.name}"
    gh secret set ECS_TASK_EXECUTION_ROLE_ARN -b"${aws_iam_role.ecs_task_execution_role.arn}"
    gh secret set ECS_TASK_ROLE_ARN -b"${aws_iam_role.ecs_task_role.arn}"
    gh secret set ECS_SECURITY_GROUP_ID -b"${aws_security_group.fargate_sg.id}"
    gh secret set ECS_SUBNET_IDS -b"${join(",", data.aws_subnets.default.ids)}"
  EOT
}

# ============================================
# Utility Commands
# ============================================

output "docker_login_command" {
  description = "Command to authenticate Docker with ECR"
  value       = "aws ecr get-login-password --region ${var.aws_region} | docker login --username AWS --password-stdin ${data.aws_caller_identity.current.account_id}.dkr.ecr.${var.aws_region}.amazonaws.com"
}

output "debug_info" {
  description = "Additional debug information"
  value = {
    account_id                    = data.aws_caller_identity.current.account_id
    region                        = data.aws_region.current.name
    github_actions_user           = aws_iam_user.github_actions.name
    github_actions_user_arn       = aws_iam_user.github_actions.arn
    ecs_cluster_name              = aws_ecs_cluster.bench_cluster.name
    ecs_task_execution_role_name  = aws_iam_role.ecs_task_execution_role.name
    ecs_task_role_name            = aws_iam_role.ecs_task_role.name
    default_vpc_id                = data.aws_vpc.default.id
    default_subnet_count          = length(data.aws_subnets.default.ids)
  }
}
