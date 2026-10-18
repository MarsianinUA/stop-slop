// @path src/orders/dto/order.response.dto.ts
import { ApiProperty, ApiPropertyOptional } from '@nestjs/swagger';

import { ApiPropertyOptional as LocalOptional } from './local-swagger';

export class OrderResponseDto {
  @ApiProperty({ type: String, nullable: true })
  note!: string | null;

  @ApiProperty({ type: Number, required: true })
  total!: number;

  @LocalOptional()
  legacy?: string;
}

export class ListOrdersQueryDto {
  @ApiPropertyOptional({ type: Number })
  limit?: number;
}

export class CreateOrderRequestDto {
  @ApiPropertyOptional()
  comment?: string;
}
