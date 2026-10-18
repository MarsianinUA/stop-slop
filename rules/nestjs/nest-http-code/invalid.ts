// @path src/orders/orders.controller.ts
// @alias @/*
import { Body, Controller, Delete, Get, Header, Param, Patch, Post } from '@nestjs/common';
import { ApiOkResponse } from '@nestjs/swagger';

import { AdminEndpoint } from '@/http/admin-endpoint.decorator';
import { Order, OrderInput } from './order.types';

@Controller('orders')
export class OrdersController {
  @Get() // expect: nest-http-code
  list(): Order[] {
    return [];
  }

  @Get(':id') // expect: nest-http-code
  @ApiOkResponse({ type: Order })
  find(@Param('id') id: string): Order {
    return { id };
  }

  @Post() // expect: nest-http-code
  create(@Body() input: OrderInput): Order {
    return { id: input.id };
  }

  @Header('Cache-Control', 'no-store') @Get('fresh') // expect: nest-http-code
  fresh(): Order[] {
    return [];
  }

  @Get('recent') @Header('Cache-Control', 'no-store') // expect: nest-http-code
  recent(): Order[] {
    return [];
  }

  @AdminEndpoint()
  @Post('import') // expect: nest-http-code
  importAll(@Body() orders: Order[]): Order[] {
    return orders;
  }

  @Get('audit') // expect: nest-http-code
  @AdminEndpoint()
  @ApiOkResponse({ type: [Order] })
  audit(): Order[] {
    return [];
  }

  @Get('ping') @AdminEndpoint() ping(): string { // expect: nest-http-code
    return 'pong';
  }

  @Delete(':id') // expect: nest-http-code
  remove(@Param('id') id: string): void {
    void id;
  }

  @Patch(':id') // expect: nest-http-code
  update(@Param('id') id: string, @Body() input: OrderInput): Order {
    return { id: input.id ?? id };
  }

  @Get('search') // expect: nest-http-code
  @Post('search')
  search(@Body() input: OrderInput): Order[] {
    return [{ id: input.id }];
  }
}
