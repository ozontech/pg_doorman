terraform {
  required_version = ">= 1.0"

  required_providers {
    aws = {
      source  = "hashicorp/aws"
      version = "~> 5.0"
    }
  }
  backend "local" {
    path = "terraform.tfstate"
  }
}

provider "aws" {
  region = var.aws_region
}

resource "aws_iam_user" "github_actions" {
  name = var.github_actions_user_name

  tags = {
    Purpose = "GitHub Actions CI/CD"
    Project = "pg_doorman"
  }
}

resource "aws_iam_access_key" "github_actions" {
  user = aws_iam_user.github_actions.name
}

# ECR Access Policy
resource "aws_iam_user_policy" "ecr_policy" {
  name = "ECRAccessPolicy"
  user = aws_iam_user.github_actions.name

  policy = jsonencode({
    Version = "2012-10-17"
    Statement = [
      {
        Sid    = "ECRAuthToken"
        Effect = "Allow"
        Action = [
          "ecr:GetAuthorizationToken"
        ]
        Resource = "*"
      },
      {
        Sid    = "ECRPushPull"
        Effect = "Allow"
        Action = [
          "ecr:BatchCheckLayerAvailability",
          "ecr:GetDownloadUrlForLayer",
          "ecr:BatchGetImage",
          "ecr:PutImage",
          "ecr:InitiateLayerUpload",
          "ecr:UploadLayerPart",
          "ecr:CompleteLayerUpload",
          "ecr:DescribeRepositories",
          "ecr:ListImages",
          "ecr:DescribeImages"
        ]
        Resource = aws_ecr_repository.pg_doorman_bench.arn
      }
    ]
  })
}

# ECS/Fargate Access Policy
resource "aws_iam_user_policy" "ecs_policy" {
  name = "ECSFargateAccessPolicy"
  user = aws_iam_user.github_actions.name

  policy = jsonencode({
    Version = "2012-10-17"
    Statement = [
      {
        Sid    = "ECSTaskManagement"
        Effect = "Allow"
        Action = [
          "ecs:RunTask",
          "ecs:StopTask",
          "ecs:DescribeTasks",
          "ecs:DescribeTaskDefinition",
          "ecs:RegisterTaskDefinition",
          "ecs:DeregisterTaskDefinition",
          "ecs:ListTasks",
          "ecs:ListTaskDefinitions"
        ]
        Resource = "*"
      },
      {
        Sid    = "IAMPassRole"
        Effect = "Allow"
        Action = [
          "iam:PassRole"
        ]
        Resource = [
          aws_iam_role.ecs_task_execution_role.arn,
          aws_iam_role.ecs_task_role.arn
        ]
      },
      {
        Sid    = "EC2NetworkAccess"
        Effect = "Allow"
        Action = [
          "ec2:DescribeSubnets",
          "ec2:DescribeSecurityGroups",
          "ec2:DescribeVpcs"
        ]
        Resource = "*"
      }
    ]
  })
}

# CloudWatch Logs Access Policy
resource "aws_iam_user_policy" "logs_policy" {
  name = "CloudWatchLogsReadPolicy"
  user = aws_iam_user.github_actions.name

  policy = jsonencode({
    Version = "2012-10-17"
    Statement = [
      {
        Sid    = "CloudWatchLogsRead"
        Effect = "Allow"
        Action = [
          "logs:DescribeLogGroups",
          "logs:DescribeLogStreams",
          "logs:GetLogEvents",
          "logs:FilterLogEvents",
          "logs:CreateLogGroup",
          "logs:CreateLogStream",
          "logs:PutLogEvents"
        ]
        Resource = [
          aws_cloudwatch_log_group.ecs_logs.arn,
          "${aws_cloudwatch_log_group.ecs_logs.arn}:*"
        ]
      }
    ]
  })
}

resource "aws_ecr_repository" "pg_doorman_bench" {
  name                 = var.ecr_repository_name
  image_tag_mutability = "MUTABLE"

  image_scanning_configuration {
    scan_on_push = true
  }

  encryption_configuration {
    encryption_type = "AES256"
  }

  tags = {
    Project = "pg_doorman"
    Purpose = "benchmark-tests"
  }
}

resource "aws_ecr_lifecycle_policy" "pg_doorman_bench_policy" {
  repository = aws_ecr_repository.pg_doorman_bench.name

  policy = jsonencode({
    rules = [
      {
        rulePriority = 1
        description  = "Keep last 10 images"
        selection = {
          tagStatus     = "any"
          countType     = "imageCountMoreThan"
          countNumber   = 10
        }
        action = {
          type = "expire"
        }
      }
    ]
  })
}

# ============================================
# ECS Cluster
# ============================================

resource "aws_ecs_cluster" "bench_cluster" {
  name = var.ecs_cluster_name

  setting {
    name  = "containerInsights"
    value = "enabled"
  }

  tags = {
    Project = "pg_doorman"
    Purpose = "benchmark-tests"
  }
}

# ============================================
# ECS Task Execution Role (for pulling images, logs)
# ============================================

resource "aws_iam_role" "ecs_task_execution_role" {
  name = "${var.ecs_cluster_name}-execution-role"

  assume_role_policy = jsonencode({
    Version = "2012-10-17"
    Statement = [
      {
        Effect = "Allow"
        Principal = {
          Service = "ecs-tasks.amazonaws.com"
        }
        Action = "sts:AssumeRole"
      }
    ]
  })

  tags = {
    Project = "pg_doorman"
    Purpose = "ecs-task-execution"
  }
}

resource "aws_iam_role_policy_attachment" "ecs_task_execution_policy" {
  role       = aws_iam_role.ecs_task_execution_role.name
  policy_arn = "arn:aws:iam::aws:policy/service-role/AmazonECSTaskExecutionRolePolicy"
}

# Additional policy for ECR access
resource "aws_iam_role_policy" "ecs_ecr_policy" {
  name = "ECRAccessPolicy"
  role = aws_iam_role.ecs_task_execution_role.id

  policy = jsonencode({
    Version = "2012-10-17"
    Statement = [
      {
        Effect = "Allow"
        Action = [
          "ecr:GetAuthorizationToken",
          "ecr:BatchCheckLayerAvailability",
          "ecr:GetDownloadUrlForLayer",
          "ecr:BatchGetImage"
        ]
        Resource = "*"
      }
    ]
  })
}

# ============================================
# ECS Task Role (for running tasks)
# ============================================

resource "aws_iam_role" "ecs_task_role" {
  name = "${var.ecs_cluster_name}-task-role"

  assume_role_policy = jsonencode({
    Version = "2012-10-17"
    Statement = [
      {
        Effect = "Allow"
        Principal = {
          Service = "ecs-tasks.amazonaws.com"
        }
        Action = "sts:AssumeRole"
      }
    ]
  })

  tags = {
    Project = "pg_doorman"
    Purpose = "ecs-task"
  }
}

# Allow task to write logs
resource "aws_iam_role_policy" "ecs_task_logs_policy" {
  name = "CloudWatchLogsPolicy"
  role = aws_iam_role.ecs_task_role.id

  policy = jsonencode({
    Version = "2012-10-17"
    Statement = [
      {
        Effect = "Allow"
        Action = [
          "logs:CreateLogGroup",
          "logs:CreateLogStream",
          "logs:PutLogEvents"
        ]
        Resource = "${aws_cloudwatch_log_group.ecs_logs.arn}:*"
      }
    ]
  })
}

# ============================================
# CloudWatch Log Group for ECS
# ============================================

resource "aws_cloudwatch_log_group" "ecs_logs" {
  name              = "/ecs/${var.ecs_cluster_name}"
  retention_in_days = var.log_retention_days

  tags = {
    Project = "pg_doorman"
    Purpose = "ecs-logs"
  }
}

# ============================================
# Default VPC and Subnet (for Fargate networking)
# ============================================

data "aws_vpc" "default" {
  default = true
}

data "aws_subnets" "default" {
  filter {
    name   = "vpc-id"
    values = [data.aws_vpc.default.id]
  }
}

# ============================================
# Security Group for Fargate tasks
# ============================================

resource "aws_security_group" "fargate_sg" {
  name        = "${var.ecs_cluster_name}-sg"
  description = "Security group for Fargate benchmark tasks"
  vpc_id      = data.aws_vpc.default.id

  # Allow all outbound traffic (needed for pulling images, etc)
  egress {
    from_port   = 0
    to_port     = 0
    protocol    = "-1"
    cidr_blocks = ["0.0.0.0/0"]
  }

  tags = {
    Project = "pg_doorman"
    Purpose = "fargate-benchmark"
  }
}

data "aws_caller_identity" "current" {}

data "aws_region" "current" {}
