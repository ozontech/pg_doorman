variable "aws_region" {
  description = "AWS region for deployment"
  type        = string
  default     = "us-east-1"

  validation {
    condition     = can(regex("^[a-z]{2}-[a-z]+-[0-9]$", var.aws_region))
    error_message = "AWS region must be in format: us-east-1, eu-west-1, etc."
  }
}

variable "github_actions_user_name" {
  description = "IAM user name for GitHub Actions"
  type        = string
  default     = "github-actions-pg-doorman"

  validation {
    condition     = length(var.github_actions_user_name) > 0 && length(var.github_actions_user_name) <= 64
    error_message = "User name must be 1-64 characters."
  }
}

variable "ecr_repository_name" {
  description = "ECR repository name for Docker images"
  type        = string
  default     = "pg-doorman-bench"

  validation {
    condition     = can(regex("^[a-z0-9]+(?:[._-][a-z0-9]+)*$", var.ecr_repository_name))
    error_message = "Repository name can only contain lowercase letters, numbers, hyphens, underscores and dots."
  }
}

variable "ecs_cluster_name" {
  description = "ECS cluster name for running benchmark tasks"
  type        = string
  default     = "pg-doorman-bench"

  validation {
    condition     = length(var.ecs_cluster_name) > 0 && length(var.ecs_cluster_name) <= 255
    error_message = "Cluster name must be 1-255 characters."
  }
}

variable "log_retention_days" {
  description = "CloudWatch logs retention in days"
  type        = number
  default     = 7

  validation {
    condition     = contains([1, 3, 5, 7, 14, 30, 60, 90, 120, 150, 180, 365, 400, 545, 731, 1827, 3653], var.log_retention_days)
    error_message = "Valid values: 1, 3, 5, 7, 14, 30, 60, 90, 120, 150, 180, 365, 400, 545, 731, 1827, 3653."
  }
}

variable "tags" {
  description = "Additional tags for all resources"
  type        = map(string)
  default = {
    Project     = "pg_doorman"
    ManagedBy   = "Terraform"
    Environment = "ci"
  }
}
