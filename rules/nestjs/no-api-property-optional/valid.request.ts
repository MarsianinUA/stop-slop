// @path src/orders/dto/list-orders.request.dto.ts
import { ApiPropertyOptional } from '@nestjs/swagger';

export class ListOrdersDto {
  @ApiPropertyOptional({ type: String })
  cursor?: string;
}

export class PageDto {
  @ApiPropertyOptional({ type: Number })
  page?: number;
}
