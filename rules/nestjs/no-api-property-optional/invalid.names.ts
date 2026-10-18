// @path src/shared/dto/api-error.dto.ts
import { ApiPropertyOptional } from '@nestjs/swagger';

export class ApiErrorResponseDto {
  @ApiPropertyOptional() // expect: no-api-property-optional
  details?: string;
}

export class UpdateOrderResponse {
  @ApiPropertyOptional() // expect: no-api-property-optional
  changed?: boolean;
}
